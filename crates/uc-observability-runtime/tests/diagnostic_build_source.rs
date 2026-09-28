//! 本地诊断记录与状态中的构建来源必须来自已链接的产物，而不是运行期所在目录的 git 状态。
//!
//! 设置 `UC_EXPECTED_SOURCE_COMMIT`/`UC_EXPECTED_SOURCE_STATE` 时按其断言，可把已编译的测试二进制
//! 复制到源码树外或在 HEAD 变化后运行，验证不会误报；未设置时回退到 CI 显式提供的构建来源。
use serde_json::Value;
use std::time::Duration;
use uc_observability_contract::diagnostics::{
    complete_operation, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, OperationCompletion,
};
use uc_observability_runtime::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, SignalResult,
};

fn valid_commit(value: &str) -> bool {
    value == "unknown" || (value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn expected(primary: &str, fallback: &str) -> Option<String> {
    std::env::var(primary)
        .ok()
        .or_else(|| std::env::var(fallback).ok())
        .filter(|value| !value.is_empty())
}

#[test]
fn local_records_and_status_report_the_linked_build_source() {
    let logs = tempfile::tempdir().expect("logs");
    let runtime = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(logs.path())),
    )
    .expect("runtime");
    complete_operation(OperationCompletion::succeeded(
        DiagnosticDomain::Storage,
        DiagnosticOperation::ProfileStorageUpgrade,
        DiagnosticRole::Local,
        Duration::from_millis(8),
    ));
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let status = runtime.handle().query_local_diagnostic_status();
    let records: Vec<Value> = std::fs::read_dir(logs.path())
        .expect("files")
        .flat_map(|entry| {
            std::fs::read_to_string(entry.expect("entry").path())
                .expect("file")
                .lines()
                .map(|line| serde_json::from_str(line).expect("record"))
                .collect::<Vec<_>>()
        })
        .filter(|record: &Value| record.get("source_commit").is_some())
        .collect();

    assert!(!records.is_empty(), "本地诊断文件应包含带来源的记录");
    assert!(
        valid_commit(&status.source_commit),
        "来源只能是完整提交号或 unknown：{}",
        status.source_commit
    );
    let state = records[0]["source_state"].as_str().expect("source_state");
    assert!(matches!(state, "clean" | "modified" | "unknown"), "{state}");
    for record in &records {
        assert_eq!(record["source_commit"], status.source_commit.as_str());
        assert_eq!(record["source_state"], state);
    }
    if let Some(commit) = expected("UC_EXPECTED_SOURCE_COMMIT", "UC_ENGINE_SOURCE_COMMIT") {
        assert_eq!(status.source_commit, commit);
    }
    if let Some(expected_state) = expected("UC_EXPECTED_SOURCE_STATE", "UC_ENGINE_SOURCE_STATE") {
        assert_eq!(state, expected_state);
    }
}
