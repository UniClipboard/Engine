use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, managed_log_file_date, managed_log_file_name, operation_span,
    scope_space_admission_observation, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
    TELEMETRY_SCHEMA_VERSION, TELEMETRY_TARGET,
};

#[test]
fn space_admission_observation_scope_returns_the_business_result() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime");
    let result = runtime.block_on(scope_space_admission_observation(&[7; 32], async { 7 }));

    assert_eq!(result, 7);
}

#[test]
fn schema_is_fixed_and_operation_api_accepts_only_typed_attributes() {
    assert_eq!(TELEMETRY_SCHEMA_VERSION, 1);
    assert_eq!(TELEMETRY_TARGET, "uc.telemetry");

    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::SpaceAdmission,
        role: DiagnosticRole::Joiner,
        kind: DiagnosticSpanKind::Client,
    });
    let _entered = span.enter();
    complete_operation(OperationCompletion::failed(
        DiagnosticDomain::SpaceAdmission,
        DiagnosticOperation::SpaceAdmission,
        DiagnosticRole::Joiner,
        DiagnosticErrorType::AuthenticationFailed,
        Duration::from_millis(21),
    ));
}

#[test]
fn managed_log_name_has_one_strict_round_trip_contract() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 5).expect("valid date");
    assert_eq!(managed_log_file_name(date), "engine.2026-09-05.jsonl");
    assert_eq!(managed_log_file_date("engine.2026-09-05.jsonl"), Some(date));
    assert!(managed_log_file_date("engine.latest.jsonl").is_none());
    assert!(managed_log_file_date("nested/engine.2026-09-05.jsonl").is_none());
}
