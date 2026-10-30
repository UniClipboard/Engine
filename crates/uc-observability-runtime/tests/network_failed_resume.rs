//! 失败清单：重开失败后刷新不得重新取得租约；真实恢复只能发生在后续重开请求中。
#![cfg(unix)]

use std::process::Command;
use std::time::Duration;

use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime, SignalResult,
};

fn lease_is_available(path: &std::path::Path) -> bool {
    let status = Command::new("python3")
        .args([
            "-c",
            "import fcntl,sys
f=open(sys.argv[1], 'r+')
try:
 fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
 sys.exit(1)",
        ])
        .arg(path)
        .status()
        .expect("run independent lease probe");
    assert!(
        matches!(status.code(), Some(0 | 1)),
        "lease probe failed unexpectedly"
    );
    status.success()
}

#[test]
fn failed_resume_cannot_reopen_a_network_lease_during_flush() {
    let root = tempfile::tempdir().expect("temporary log root");
    let budget = NetworkLogBudget {
        record_bytes: 1024,
        queue_records: 32,
        file_bytes: 4096,
        run_bytes: 8192,
        root_bytes: 16384,
        retention: Duration::from_secs(60),
    };
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_network_logs(
        NetworkLogConfig::new(root.path(), NetworkProcessRole::MobileApp, budget)
            .expect("network configuration"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut initial, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        initial.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let network = root.path().join("network");
    std::fs::remove_dir_all(&network).expect("remove parked network directory");
    std::fs::write(&network, b"block directory creation").expect("block network directory");

    let (mut failed, result) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(result, Some(SignalResult::Failed));
    std::fs::remove_file(&network).expect("restore network path");
    // 进程刷新不经过网络导出的状态检查，必须同样保持停车状态。
    let _ = handle.force_flush(Duration::from_secs(5));
    let export = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export");
    assert!(
        !export.run_directory.join("lease.lock").exists(),
        "flush reopened the lease after a failed resume"
    );

    assert_eq!(
        failed.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let (mut resumed, result) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(result, Some(SignalResult::Completed));
    let lease = export.run_directory.join("lease.lock");
    assert!(lease.exists(), "explicit resume did not reopen the lease");
    assert!(
        !lease_is_available(&lease),
        "independent process acquired an active lease"
    );
    assert_eq!(
        resumed.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    assert!(
        lease_is_available(&lease),
        "independent process could not acquire the released lease"
    );
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("create evidence directory");
        let result = serde_json::json!({
            "scenario": "network_failed_resume",
            "scope": "real network files and independent flock process",
            "failed_resume": "failed",
            "lease_after_flush": "absent",
            "lease_after_explicit_resume": "held",
            "lease_after_park": "available"
        });
        std::fs::write(
            std::path::Path::new(&directory).join("network-failed-resume.json"),
            serde_json::to_vec_pretty(&result).expect("serialize evidence"),
        )
        .expect("write evidence");
    }
    let _ = handle.shutdown(Duration::from_secs(5));
}
