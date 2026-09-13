//! 实际普通模式文件验收：阶段关联、耗时、原始错误保留、中断与去重。
use serde_json::Value;
use std::time::Duration;
use tracing::Instrument;
use uc_observability_contract::diagnostics::connectivity::{
    observe_local_result, scope_pairing_work, AdmissionExchangeSide, LocalWorkOutcome,
    LocalWorkStep, MaintenanceDisposition, MaintenanceObservation, RecoveryTrigger,
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
async fn local_work_records_duration_failure_cancellation_and_queue_in_standard_files() {
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
        .scope(async {
            for action in [
                AdmissionObservationAction::RequestJoin,
                AdmissionObservationAction::ConfirmPrepared,
                AdmissionObservationAction::ConfirmApplied,
                AdmissionObservationAction::Settle,
            ] {
                scope_pairing_work(AdmissionExchangeSide::Joiner, Some(action), async {
                    observe_local_result(LocalWorkStep::JoinerStateCommit, async {
                        Ok::<_, ()>(())
                    })
                    .await
                    .expect("save");
                })
                .await;
            }
            scope_pairing_work(
                AdmissionExchangeSide::Sponsor,
                Some(AdmissionObservationAction::RequestJoin),
                async {
                    let result = observe_local_result(LocalWorkStep::SponsorStateLoad, async {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Err::<(), _>("PRIVATE_KEY_INVITATION_AND_PATH")
                    })
                    .await;
                    assert_eq!(
                        result,
                        Err("PRIVATE_KEY_INVITATION_AND_PATH"),
                        "日志不能替换原始错误"
                    );
                    assert!(tokio::time::timeout(
                        Duration::from_millis(10),
                        observe_local_result(
                            LocalWorkStep::SponsorPrepareCandidate,
                            std::future::pending::<Result<(), ()>>()
                        )
                    )
                    .await
                    .is_err());
                },
            )
            .await;
            let mut round = MaintenanceObservation::request(RecoveryTrigger::StateChanged);
            round.queued();
            MaintenanceObservation::request(RecoveryTrigger::StateChanged).coalesce(&round);
            MaintenanceObservation::request(RecoveryTrigger::Periodic)
                .not_executed(MaintenanceDisposition::Paused);
            tokio::time::sleep(Duration::from_millis(20)).await;
            round.start();
            round
                .scope(observe_local_result(
                    LocalWorkStep::MaintenanceEffects,
                    async { Ok::<_, ()>(()) },
                ))
                .await
                .expect("effects");
            round.finish(LocalWorkOutcome::Deferred);
        })
        .instrument(span)
        .await;
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
    let diagnostics: Vec<_> = records
        .iter()
        .filter(|r| r["target"] == "uc.connectivity")
        .collect();
    assert!(!diagnostics.is_empty());
    for record in &diagnostics {
        assert_eq!(record["capture_mode"], "standard");
        assert!(record["trace_id"].is_string(), "关联不能为空：{record:?}");
        assert_eq!(record["trace_id"], diagnostics[0]["trace_id"]);
        assert_eq!(record["span_id"], diagnostics[0]["span_id"]);
        assert!(!record.to_string().contains("PRIVATE_"));
    }
    let finished: Vec<_> = diagnostics
        .iter()
        .filter(|r| r["fields"]["event.name"] == "runtime.work.finished")
        .collect();
    assert_eq!(finished.len(), 7);
    for (index, message) in ["join_request", "prepared", "applied", "complete_ack"]
        .into_iter()
        .enumerate()
    {
        let record = finished
            .iter()
            .find(|r| r["fields"]["message"] == message && r["fields"]["uc.role"] == "joiner")
            .expect("round");
        assert_eq!(record["fields"]["protocol_round"], index + 1);
        assert_eq!(record["fields"]["uc.outcome"], "ok");
    }
    let failed = finished
        .iter()
        .find(|r| r["fields"]["step"] == "sponsor_state_load")
        .expect("failure");
    assert_eq!(failed["fields"]["uc.outcome"], "error");
    assert!(failed["fields"]["duration_ms"].as_u64().expect("duration") >= 30);
    let cancelled = finished
        .iter()
        .find(|r| r["fields"]["step"] == "sponsor_prepare_candidate")
        .expect("cancelled");
    assert_eq!(cancelled["fields"]["uc.outcome"], "interrupted");
    let start = diagnostics
        .iter()
        .find(|r| r["fields"]["event.name"] == "membership.maintenance.started")
        .expect("started");
    assert!(start["fields"]["queue_wait_ms"].as_u64().expect("wait") >= 20);
    let effects = finished
        .iter()
        .find(|r| r["fields"]["step"] == "maintenance_effects")
        .expect("effects");
    assert_eq!(
        effects["fields"]["maintenance_round"],
        start["fields"]["maintenance_round"]
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|r| r["fields"]["event.name"] == "membership.maintenance.started")
            .count(),
        1
    );
}
