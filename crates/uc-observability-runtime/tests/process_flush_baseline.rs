//! 真实进程文件基线：刷新完成不代表日志文件描述符已经关闭。
//! 失败方式：刷新未完成、没有创建日志文件、独立进程无法观察描述符，或基线证据不能写入。
#![cfg(target_os = "macos")]

use std::process::Command;
use std::time::Duration;

use uc_observability_contract::uc_warn;
use uc_observability_runtime::{
    managed_log_files, DeploymentEnvironment, LocalLogConfig, ObservabilityConfig,
    ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime, SignalResult,
};

#[test]
fn flush_keeps_the_real_daily_file_open() {
    let root = tempfile::tempdir().expect("temporary log root");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.0.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Other,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(root.path())),
    )
    .expect("install")
    .handle();
    uc_warn!(target: "uc_infra_profile::release_baseline", error_kind = "baseline", "release baseline");
    let flush = ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5));
    assert_eq!(flush, SignalResult::Completed);
    let files = managed_log_files(root.path()).expect("list logs");
    assert!(!files.is_empty(), "daily log was not created");
    let output = Command::new("lsof")
        .args(["-n", "-P", "-F", "n", "-p", &std::process::id().to_string()])
        .output()
        .expect("run independent descriptor probe");
    assert!(output.status.success(), "descriptor probe failed");
    let names = String::from_utf8(output.stdout).expect("probe output");
    let open = files.iter().any(|path| {
        let canonical = path.canonicalize().expect("canonical log path");
        names
            .lines()
            .any(|line| line.strip_prefix('n') == canonical.to_str())
    });
    assert!(
        open,
        "flush did not preserve the expected baseline descriptor"
    );
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("create evidence directory");
        let result = serde_json::json!({
            "scenario": "process_flush_baseline",
            "scope": "real daily file and independent lsof process",
            "flush": "completed",
            "daily_descriptor_after_flush": "open",
            "process_release": "unsupported",
        });
        std::fs::write(
            std::path::Path::new(&directory).join("process-flush-baseline.json"),
            serde_json::to_vec_pretty(&result).expect("serialize evidence"),
        )
        .expect("write evidence");
    }
    let _ = handle.shutdown(Duration::from_secs(5));
}
