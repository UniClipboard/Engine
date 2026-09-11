use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_contract::diagnostics::*;
use uc_observability_runtime::*;

#[test]
fn member_update_detail_is_consumed_once_into_the_actual_local_file() {
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
    let completion = || {
        OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            DiagnosticOperation::MembershipGroupUpdate,
            DiagnosticRole::Member,
            DiagnosticErrorType::Storage,
            Duration::from_millis(4),
        )
    };
    complete_group_update_failure(
        GroupUpdateFailureDetail {
            phase: GroupUpdatePhase::PersistState,
            reason: GroupUpdateReason::PermissionDenied,
            source: GroupUpdateSource::Io,
        },
        completion(),
    );
    complete_operation(completion());
    assert_eq!(
        handle.force_flush(Duration::from_secs(2)).logs,
        SignalResult::Completed
    );
    let rows: Vec<serde_json::Value> = managed_log_files(directory.path())
        .expect("files")
        .iter()
        .flat_map(|file| {
            std::fs::read_to_string(file)
                .expect("content")
                .lines()
                .map(|line| serde_json::from_str(line).expect("JSON"))
                .filter(|row: &serde_json::Value| row["target"] != "uc.diagnostics")
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["fields"]["error.phase"], "persist_state");
    assert_eq!(rows[0]["fields"]["error.reason"], "permission_denied");
    assert_eq!(
        rows[0]["fields"]["error.chain"],
        serde_json::json!([
            "membership_update",
            "persist_state",
            "io",
            "permission_denied"
        ])
    );
    assert!(rows[1]["fields"].get("error.phase").is_none());
    assert_eq!(rows[0]["run_id"], rows[1]["run_id"]);
    handle.shutdown(Duration::from_secs(2));
}
