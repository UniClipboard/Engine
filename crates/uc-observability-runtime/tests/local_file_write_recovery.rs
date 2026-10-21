//! 本地文件写入失败后必须可恢复，健康状态随之动态变化。
//!
//! 真实故障：把日志目录替换为同名普通文件，目录无法重建，写入与重新打开都返回 I/O 错误；
//! 随后移除该文件，模拟存储恢复。旧实现一次写入错误就永久标记失败，之后所有记录和导出刷新都失败，
//! 而 `health()` 仍报告就绪。
//!
//! 预先定义的失败方式（任一发生即测试失败）：
//! 1. 故障期间 `health().local_file` 仍是 `Ready`，或故障期间的刷新报告成功；
//! 2. 故障期间的丢弃没有计数；
//! 3. 存储恢复后刷新仍然失败、新记录没有落盘，或健康状态没有回到 `Ready`；
//! 4. 恢复时没有留下健康记录，或该记录缺少 `run_id`、`local_schema_version`、`source_commit` 信封，
//!    导致无法与同一次运行的其他记录对应。
//!
//! 运行期只能安装一次，所以所有阶段放在同一个测试里依次执行。
use std::path::Path;
use std::time::Duration;

use uc_observability_contract::uc_warn;
use uc_observability_runtime::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, SetupStatus, SignalResult,
};

fn rows(directory: &Path) -> Vec<serde_json::Value> {
    let Ok(files) = uc_observability_runtime::managed_log_files(directory) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path).expect("log file");
        rows.extend(
            text.lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON line")),
        );
    }
    rows
}

fn event_count(directory: &Path, name: &str) -> usize {
    rows(directory)
        .iter()
        .filter(|row| row["fields"]["event.name"] == name)
        .count()
}

fn has_marker(directory: &Path, kind: &str) -> bool {
    rows(directory)
        .iter()
        .any(|row| row["fields"]["error_kind"] == kind)
}

#[test]
fn writer_recovers_after_storage_failure_and_health_follows() {
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
    let flush = || ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5));

    uc_warn!(target: "uc_infra_profile::write_recovery", error_kind = "before_failure", "write recovery marker");
    assert_eq!(flush(), SignalResult::Completed);
    assert!(has_marker(&directory, "before_failure"));
    assert_eq!(handle.health().local_file, SetupStatus::Ready);

    std::fs::remove_dir_all(&directory).expect("remove directory");
    std::fs::write(&directory, b"not a directory").expect("block the path with a file");
    uc_warn!(target: "uc_infra_profile::write_recovery", error_kind = "during_failure", "write recovery marker");
    assert_eq!(flush(), SignalResult::Failed, "flush during the failure");
    let failing = handle.health();
    assert_eq!(failing.local_file, SetupStatus::Unavailable);
    assert!(
        failing.dropped_local_records >= 1,
        "records lost during the failure are counted"
    );

    std::fs::remove_file(&directory).expect("storage recovers");
    // 失败后按退避重试；等待超过当前退避，之后到达的记录应当写入。
    std::thread::sleep(Duration::from_millis(500));
    uc_warn!(target: "uc_infra_profile::write_recovery", error_kind = "after_recovery", "write recovery marker");
    assert_eq!(flush(), SignalResult::Completed, "flush after recovery");
    assert!(has_marker(&directory, "after_recovery"));
    assert!(!has_marker(&directory, "during_failure"));
    assert_eq!(handle.health().local_file, SetupStatus::Ready);
    // 恢复记录由文件线程重新入队，可能排在第一次刷新之后。再次刷新保证该记录也已落盘。
    assert_eq!(flush(), SignalResult::Completed, "flush recovery record");
    assert_eq!(
        event_count(&directory, "uc.observability.local_file_recovered"),
        1
    );
    let all = rows(&directory);
    let recovered = all
        .iter()
        .find(|row| row["fields"]["event.name"] == "uc.observability.local_file_recovered")
        .expect("recovered record");
    let marker = all
        .iter()
        .find(|row| row["fields"]["error_kind"] == "after_recovery")
        .expect("marker record");
    assert_eq!(recovered["local_schema_version"], 2);
    assert!(
        recovered["run_id"].is_string(),
        "health record carries run_id"
    );
    assert_eq!(
        recovered["run_id"], marker["run_id"],
        "same run as ordinary records"
    );
    assert_eq!(recovered["source_commit"], marker["source_commit"]);
    assert_eq!(recovered["engine_version"], marker["engine_version"]);
}
