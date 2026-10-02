//! 安全存储用量随本地工作完成记录输出：子工作计入外层，没有读取时不出现字段。
use std::time::Duration;

use serde_json::Value;
use uc_observability_contract::diagnostics::connectivity::{
    observe_local_result, observe_local_sync_result, record_secure_storage_read,
    scope_pairing_work, AdmissionExchangeSide, LocalWorkStep,
};
use uc_observability_contract::diagnostics::{
    operation_span, AdmissionObservationAction, DiagnosticDomain, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, ObservationContext, OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, SignalResult,
};

#[tokio::test]
async fn secure_storage_use_is_reported_per_step_and_inclusive_of_children() {
    let logs = tempfile::tempdir().expect("logs");
    let _runtime = ProcessObservabilityRuntime::install(
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
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::NetworkTransport,
        role: DiagnosticRole::Joiner,
        kind: DiagnosticSpanKind::Client,
    });
    let context = span.in_scope(ObservationContext::capture);
    context
        .scope(scope_pairing_work(
            AdmissionExchangeSide::Joiner,
            Some(AdmissionObservationAction::RequestJoin),
            async {
                observe_local_result(LocalWorkStep::JoinerStateLoad, async { Ok::<_, ()>(()) })
                    .await
                    .expect("state load");
                observe_local_result(LocalWorkStep::JoinerStateCommit, async {
                    record_secure_storage_read(Duration::from_millis(4));
                    observe_local_sync_result(LocalWorkStep::RepositorySave, || {
                        record_secure_storage_read(Duration::from_millis(3));
                        record_secure_storage_read(Duration::from_millis(5));
                        Ok::<_, ()>(())
                    })
                    .expect("save");
                    Ok::<_, ()>(())
                })
                .await
                .expect("commit");
            },
        ))
        .await;
    // 范围外的读取不产生任何记录，也不影响后续观测。
    record_secure_storage_read(Duration::from_millis(1));
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );

    let records: Vec<Value> = std::fs::read_dir(logs.path())
        .expect("files")
        .flat_map(|entry| {
            std::fs::read_to_string(entry.expect("entry").path())
                .expect("file")
                .lines()
                .map(|line| serde_json::from_str(line).expect("record"))
                .collect::<Vec<_>>()
        })
        .collect();
    let finished = |step: &str| -> Value {
        records
            .iter()
            .find(|r| {
                r["fields"]["event.name"] == "runtime.work.finished" && r["fields"]["step"] == step
            })
            .unwrap_or_else(|| panic!("缺少完成记录：{step}"))["fields"]
            .clone()
    };
    let load = finished("joiner_state_load");
    assert!(load.get("secure_storage_reads").is_none());
    assert!(load.get("secure_storage_read_ms").is_none());
    let save = finished("repository_save");
    assert_eq!(save["secure_storage_reads"], 2);
    assert_eq!(save["secure_storage_read_ms"], 8);
    let commit = finished("joiner_state_commit");
    assert_eq!(commit["secure_storage_reads"], 3);
    assert_eq!(commit["secure_storage_read_ms"], 12);
}
