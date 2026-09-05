use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, scope_space_admission_observation, DiagnosticDomain,
    DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use crate::test_support::capture_telemetry;

#[test]
fn one_event_becomes_one_correlated_log_without_a_duplicate_span_event() {
    let captured = capture_telemetry(|| {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Storage,
            operation: DiagnosticOperation::ProfileStorageUpgrade,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
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
    assert_eq!(captured.spans[0].rejection_reason, None);
    assert_eq!(captured.logs.len(), 1);
    assert_eq!(captured.spans[0].event_count, 0);
    assert_eq!(captured.logs[0].trace_id, captured.spans[0].trace_id);
    assert_eq!(captured.logs[0].span_id, captured.spans[0].span_id);
}

#[test]
fn audited_target_rejects_an_allowlisted_event_with_a_log_body() {
    let captured = capture_telemetry(|| {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Runtime,
            operation: DiagnosticOperation::SessionLifecycle,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        });
        let _entered = span.enter();
        tracing::event!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            event.name = "uc.operation.completed",
            "PRIVATE_BODY_WITH_ALLOWLISTED_FIELDS"
        );
    });

    assert!(captured.logs.is_empty());
}

#[test]
fn repeated_execution_for_one_persisted_attempt_uses_new_traces_and_one_flow() {
    let captured = capture_telemetry(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");
        runtime.block_on(scope_space_admission_observation(&[7; 32], async {
            for _ in 0..2 {
                tokio::task::yield_now().await;
                let span = operation_span(OperationContext {
                    domain: DiagnosticDomain::SpaceAdmission,
                    operation: DiagnosticOperation::NetworkTransport,
                    role: DiagnosticRole::Joiner,
                    kind: DiagnosticSpanKind::Client,
                });
                let _entered = span.enter();
                complete_operation(OperationCompletion::succeeded(
                    DiagnosticDomain::SpaceAdmission,
                    DiagnosticOperation::NetworkTransport,
                    DiagnosticRole::Joiner,
                    Duration::ZERO,
                ));
            }
            for context in [
                OperationContext {
                    domain: DiagnosticDomain::SpaceAdmission,
                    operation: DiagnosticOperation::NetworkTransport,
                    role: DiagnosticRole::Sponsor,
                    kind: DiagnosticSpanKind::Server,
                },
                OperationContext {
                    domain: DiagnosticDomain::SpaceAdmission,
                    operation: DiagnosticOperation::SpaceAdmission,
                    role: DiagnosticRole::Sponsor,
                    kind: DiagnosticSpanKind::Internal,
                },
            ] {
                drop(operation_span(context));
            }
        }));

        let without_scope = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        drop(without_scope);

        runtime.block_on(scope_space_admission_observation(&[8; 32], async {
            let span = operation_span(OperationContext {
                domain: DiagnosticDomain::SpaceAdmission,
                operation: DiagnosticOperation::NetworkTransport,
                role: DiagnosticRole::Joiner,
                kind: DiagnosticSpanKind::Client,
            });
            drop(span);
        }));
    });

    assert_eq!(captured.spans.len(), 6);
    assert_ne!(captured.spans[0].trace_id, captured.spans[1].trace_id);
    assert_eq!(captured.spans[0].flow_id, captured.spans[1].flow_id);
    assert!(captured.spans[0].flow_id.is_some());
    assert!(captured.spans[2].flow_id.is_none());
    assert!(captured.spans[3].flow_id.is_none());
    assert!(captured.spans[4].flow_id.is_none());
    assert_ne!(captured.spans[0].flow_id, captured.spans[5].flow_id);
    assert_eq!(captured.spans[0].flow_id.as_deref().map(str::len), Some(32));
}
