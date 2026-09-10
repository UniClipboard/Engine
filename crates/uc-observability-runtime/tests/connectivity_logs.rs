use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_contract::diagnostics::ObservationContext;
use uc_observability_runtime::*;

#[test]
fn connectivity_diagnostics_survive_the_export_file_without_private_fields() {
    let directory = tempfile::tempdir().expect("logs");
    let _handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(directory.path())),
    )
    .expect("install")
    .handle();
    record_admission_recovery_decision(
        &ObservationContext::capture(),
        RecoveryTrigger::Resume,
        RecoveryDecision::Deferred(Some(RecoveryDeferral::Exchange(
            ExchangeFailure::AuthenticationRejected,
        ))),
    );
    PresenceCheckObservation::begin().finish(PresenceCheckResult::Reachable);
    record_presence_closed(
        ConnectionDirection::Outbound,
        ConnectionCloseReason::RemoteApplicationClosed,
    );
    SessionTransitionObservation::begin(SessionTransition::Suspend)
        .finish(SessionTransitionResult::Completed);
    tracing::event!(target: "uc.connectivity", tracing::Level::INFO, event.name = "presence.connection.closed", payload = "PRIVATE_REASON");
    tracing::event!(target: "uc.connectivity", tracing::Level::INFO, device = "PRIVATE_DEVICE", "PRIVATE_BODY");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let output = std::fs::read_dir(directory.path())
        .expect("files")
        .map(|e| std::fs::read_to_string(e.expect("file").path()).expect("content"))
        .collect::<String>();
    assert!(!output.contains("PRIVATE_"));
    let rows: Vec<serde_json::Value> = output
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .collect();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["fields"]["uc.outcome"], "deferred");
    assert_eq!(rows[0]["fields"]["trigger"], "resume");
    assert_eq!(rows[0]["fields"]["error.reason"], "authentication_rejected");
    assert_eq!(
        rows[0]["fields"]["next_action"],
        "wait_for_recovery_trigger"
    );
    assert_eq!(rows[0]["level"], "WARN");
    assert!(rows[0]["fields"].get("duration_ms").is_none());
    assert!(rows[1]["fields"]["duration_ms"].as_u64().is_some());
    assert_eq!(
        rows[2]["fields"]["close.reason"],
        "remote_application_closed"
    );
    assert!(rows[2]["fields"].get("duration_ms").is_none());
    assert!(rows[3]["fields"].get("duration_ms").is_none());
    assert_eq!(rows[4]["fields"]["uc.outcome"], "ok");
    assert!(rows.iter().all(|r| r["fields"].get("payload").is_none()));
}
