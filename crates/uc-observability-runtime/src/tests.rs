use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use crate::runtime::capture_telemetry;

#[test]
fn one_event_becomes_one_correlated_log_without_a_duplicate_span_event() {
    let captured = capture_telemetry(|| {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Storage,
            operation: DiagnosticOperation::ProfileStorageUpgrade,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
            flow: None,
        });
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            Duration::from_millis(12),
        ));
    });

    assert_eq!(captured.spans.len(), 1);
    assert_eq!(captured.logs.len(), 1);
    assert_eq!(captured.spans[0].event_count, 0);
    assert_eq!(captured.logs[0].trace_id, captured.spans[0].trace_id);
    assert_eq!(captured.logs[0].span_id, captured.spans[0].span_id);
}
