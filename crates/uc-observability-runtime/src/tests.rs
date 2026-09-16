use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext, SpaceAdmissionObservation,
    SpaceAdmissionObservationOutcome,
};

use crate::test_support::capture_telemetry;

#[test]
fn admission_roles_have_distinct_readable_names() {
    let captured = capture_telemetry(|| {
        let root = SpaceAdmissionObservation::begin(&[42; 32]);
        for (operation, role, kind) in [
            (
                DiagnosticOperation::SpaceAdmission,
                DiagnosticRole::Joiner,
                DiagnosticSpanKind::Client,
            ),
            (
                DiagnosticOperation::NetworkTransport,
                DiagnosticRole::Joiner,
                DiagnosticSpanKind::Client,
            ),
            (
                DiagnosticOperation::NetworkTransport,
                DiagnosticRole::Sponsor,
                DiagnosticSpanKind::Server,
            ),
            (
                DiagnosticOperation::SpaceAdmission,
                DiagnosticRole::Sponsor,
                DiagnosticSpanKind::Internal,
            ),
        ] {
            drop(operation_span(OperationContext {
                domain: DiagnosticDomain::SpaceAdmission,
                operation,
                role,
                kind,
            }));
        }
        root.finish(SpaceAdmissionObservationOutcome::Succeeded);
    });
    let names = captured
        .spans
        .iter()
        .map(|span| span.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names.len(),
        5,
        "different admission actions must have different names: {names:?}"
    );
}

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
fn lifecycle_root_and_children_share_one_approved_trace() {
    let captured = capture_telemetry(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");
        let observation = SpaceAdmissionObservation::begin(&[7; 32]);
        runtime.block_on(observation.scope(async {
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
        }));
        observation.finish(SpaceAdmissionObservationOutcome::Succeeded);
    });

    assert_eq!(captured.spans.len(), 3);
    let root = captured
        .spans
        .iter()
        .find(|span| span.parent_span_id == "0000000000000000")
        .expect("one lifecycle root");
    assert_eq!(root.operation.as_deref(), Some("space_admission"));
    assert_eq!(root.role.as_deref(), Some("local"));
    assert_eq!(root.flow_id.as_deref().map(str::len), Some(32));
    assert!(captured.spans.iter().all(|span| {
        span.trace_id == root.trace_id
            && span.rejection_reason.is_none()
            && (span.span_id == root.span_id || span.parent_span_id == root.span_id)
    }));
    assert_eq!(captured.logs.len(), 3);
    let root_log = captured
        .logs
        .iter()
        .find(|log| log.span_id == root.span_id)
        .expect("one lifecycle completion log");
    assert_eq!(root_log.outcome.as_deref(), Some("ok"));
}

#[test]
fn unfinished_lifecycle_is_deferred_when_its_last_handle_is_dropped() {
    let captured = capture_telemetry(|| {
        let observation = SpaceAdmissionObservation::begin(&[9; 32]);
        drop(observation);
    });

    assert_eq!(captured.spans.len(), 1);
    assert_eq!(captured.logs.len(), 1);
    assert_eq!(captured.logs[0].outcome.as_deref(), Some("deferred"));
    assert_eq!(captured.logs[0].trace_id, captured.spans[0].trace_id);
    assert_eq!(captured.logs[0].span_id, captured.spans[0].span_id);
}

#[test]
fn cancelled_lifecycle_keeps_its_explicit_outcome() {
    let captured = capture_telemetry(|| {
        let observation = SpaceAdmissionObservation::begin(&[10; 32]);
        observation.finish(SpaceAdmissionObservationOutcome::Cancelled);
    });

    assert_eq!(captured.spans.len(), 1);
    assert_eq!(captured.logs.len(), 1);
    assert_eq!(captured.logs[0].outcome.as_deref(), Some("cancelled"));
}

#[test]
fn shared_bridge_keeps_local_events_available_to_the_file_processor() {
    use uc_observability_contract::diagnostics::connectivity::{
        record_presence_closed, ConnectionCloseReason, ConnectionDirection,
    };
    let captured = capture_telemetry(|| {
        record_presence_closed(
            ConnectionDirection::Outbound,
            ConnectionCloseReason::RemoteApplicationClosed,
        );
    });
    assert!(captured.spans.is_empty());
    assert_eq!(
        captured.logs.len(),
        1,
        "the shared bridge must retain local events until output routing"
    );
}
