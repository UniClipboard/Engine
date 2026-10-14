//! 日志目录或当天文件在进程运行期间消失后，写入端必须重建并继续可见。
//!
//! 真实场景：移动端退出空间会清理整个缓存目录，宿主或系统清理也可能删除日志；打开的文件描述符
//! 仍然可写，旧实现因此把后续记录写进已经脱离目录的文件，导出时找不到任何记录。
//!
//! 预先定义的失败方式（任一发生即测试失败）：
//! 1. 目录被删除后新记录没有出现在重新创建的目录里；
//! 2. 只有当天文件被删除时，新记录没有出现在重新创建的文件里；
//! 3. 恢复后 `health().local_file` 不是 `Ready`，或记录被计入丢弃。
//!
//! 运行期只能安装一次，所以所有阶段放在同一个测试里依次执行。
use std::path::Path;
use std::time::Duration;

use uc_observability_contract::uc_warn;
use uc_observability_runtime::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, SetupStatus, SignalResult,
};

fn kinds(directory: &Path) -> Vec<String> {
    let Ok(files) = uc_observability_runtime::managed_log_files(directory) else {
        return Vec::new();
    };
    let mut kinds = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path).expect("log file");
        for line in text.lines() {
            let row: serde_json::Value = serde_json::from_str(line).expect("each line is JSON");
            if row["source"] == "engine_module" {
                if let Some(kind) = row["fields"]["error_kind"].as_str() {
                    kinds.push(kind.to_owned());
                }
            }
        }
    }
    kinds
}

fn flush() {
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
}

#[test]
fn writer_recreates_a_missing_directory_and_a_vanished_day_file() {
    let root = tempfile::tempdir().expect("root");
    let directory = root.path().join("logs");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(&directory)),
    )
    .expect("runtime")
    .handle();

    uc_warn!(target: "uc_infra_profile::path_loss", error_kind = "before_loss", "path loss marker");
    flush();
    assert_eq!(kinds(&directory), ["before_loss"]);

    std::fs::remove_dir_all(&directory).expect("remove log directory");
    uc_warn!(target: "uc_infra_profile::path_loss", error_kind = "after_directory_loss", "path loss marker");
    flush();
    assert_eq!(
        kinds(&directory),
        ["after_directory_loss"],
        "directory loss must not swallow later records"
    );

    for path in uc_observability_runtime::managed_log_files(&directory).expect("files") {
        std::fs::remove_file(path).expect("remove day file");
    }
    uc_warn!(target: "uc_infra_profile::path_loss", error_kind = "after_file_loss", "path loss marker");
    flush();
    assert_eq!(
        kinds(&directory),
        ["after_file_loss"],
        "file loss must not swallow later records"
    );

    let health = handle.health();
    assert_eq!(health.local_file, SetupStatus::Ready);
    assert_eq!(health.dropped_local_records, 0);
}
