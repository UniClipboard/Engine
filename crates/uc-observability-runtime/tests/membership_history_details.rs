use std::time::Duration;

use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_contract::diagnostics::*;
use uc_observability_runtime::*;

#[test]
fn history_failure_exports_a_safe_stage_error_chain_and_call_path() {
    let directory = tempfile::tempdir().expect("logs");
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
        .with_local_logs(LocalLogConfig::new(directory.path())),
    )
    .expect("install")
    .handle();
    complete_membership_history_failure(
        MembershipHistoryFailureDetail {
            phase: MembershipHistoryFailurePhase::ExchangeHistory,
            reason: MembershipHistoryFailureReason::Transport,
        },
        OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            DiagnosticOperation::MembershipHistorySync,
            DiagnosticRole::Member,
            DiagnosticErrorType::StreamFailed,
            Duration::from_millis(4),
        ),
    );
    assert_eq!(
        handle.force_flush(Duration::from_secs(2)).logs,
        SignalResult::Completed
    );
    let rows = managed_log_files(directory.path())
        .expect("files")
        .iter()
        .flat_map(|file| {
            std::fs::read_to_string(file)
                .expect("content")
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON"))
                .filter(|row| row["target"] != "uc.diagnostics")
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fields"]["error.phase"], "exchange_history");
    assert_eq!(rows[0]["fields"]["error.reason"], "transport");
    assert_eq!(
        rows[0]["fields"]["error.chain"],
        serde_json::json!([
            "space_device_update",
            "membership_history",
            "exchange_history",
            "transport"
        ])
    );
    assert_eq!(
        rows[0]["fields"]["error.call_path"],
        rows[0]["fields"]["error.chain"]
    );
    let serialized = serde_json::to_string(&rows).expect("serialize rows");
    for forbidden in [
        "device_id",
        "invitation",
        "credential",
        "signature",
        "secret",
    ] {
        assert!(!serialized.contains(forbidden));
    }
    handle.shutdown(Duration::from_secs(2));
}
