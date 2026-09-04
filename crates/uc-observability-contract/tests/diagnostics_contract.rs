use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticFlowId,
    DiagnosticFlowPurpose, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationCompletion, OperationContext, TELEMETRY_SCHEMA_VERSION, TELEMETRY_TARGET,
};

#[test]
fn diagnostic_flow_is_stable_separated_and_redacted() {
    let source = b"private-business-identifier";
    let first = DiagnosticFlowId::derive(DiagnosticFlowPurpose::SpaceAdmission, source);
    let repeated = DiagnosticFlowId::derive(DiagnosticFlowPurpose::SpaceAdmission, source);
    let other = DiagnosticFlowId::derive(DiagnosticFlowPurpose::ClipboardSync, source);

    assert_eq!(first, repeated);
    assert_ne!(first, other);
    assert!(!format!("{first:?}").contains("private-business-identifier"));
    assert_eq!(format!("{first:?}"), "DiagnosticFlowId(REDACTED)");
}

#[test]
fn schema_is_fixed_and_operation_api_accepts_only_typed_attributes() {
    assert_eq!(TELEMETRY_SCHEMA_VERSION, 1);
    assert_eq!(TELEMETRY_TARGET, "uc.telemetry");

    let flow = DiagnosticFlowId::derive(
        DiagnosticFlowPurpose::SpaceAdmission,
        b"private-admission-attempt",
    );
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::SpaceAdmission,
        role: DiagnosticRole::Joiner,
        kind: DiagnosticSpanKind::Client,
        flow: Some(&flow),
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
