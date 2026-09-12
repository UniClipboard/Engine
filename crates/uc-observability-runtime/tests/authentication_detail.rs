use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::{
    complete_admission_authentication_failure, AuthenticationFailure, CredentialFailure,
};
use uc_observability_contract::diagnostics::*;
use uc_observability_runtime::*;

#[test]
fn authentication_completion_has_one_local_detail_and_does_not_leak_to_the_next_event() {
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
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::NetworkTransport,
        role: DiagnosticRole::Sponsor,
        kind: DiagnosticSpanKind::Server,
    });
    span.in_scope(|| {
        complete_admission_authentication_failure(
            AuthenticationFailure::ContinuationCredential(CredentialFailure::RecordMissing),
            Duration::from_millis(3),
        );
        complete_operation(OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            DiagnosticOperation::NetworkTransport,
            DiagnosticRole::Sponsor,
            DiagnosticErrorType::AuthenticationFailed,
            Duration::from_millis(1),
        ));
    });
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let output = std::fs::read_dir(directory.path())
        .expect("files")
        .map(|e| std::fs::read_to_string(e.expect("file").path()).expect("content"))
        .collect::<String>();
    let rows: Vec<serde_json::Value> = output
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .filter(|row: &serde_json::Value| row["target"] != "uc.diagnostics")
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "a failure must not produce a second standalone detail event"
    );
    assert_eq!(rows[0]["fields"]["error.phase"], "continuation_credential");
    assert_eq!(rows[0]["fields"]["error.reason"], "record_missing");
    assert_eq!(rows[0]["fields"]["error.type"], "authentication_failed");
    assert!(rows[0].get("trace_id").is_none());
    assert!(
        rows[1]["trace_id"].as_str().is_some(),
        "the caller context must be restored after a detached failure"
    );
    assert!(rows[1]["fields"].get("error.reason").is_none());
}
