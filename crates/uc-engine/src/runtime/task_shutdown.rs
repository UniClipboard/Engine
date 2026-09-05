use std::time::{Duration, Instant};

use tracing::Instrument;
use uc_core::{TaskRegistry, TaskShutdownReport};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, record_task_shutdown, DiagnosticDomain,
    DiagnosticErrorType, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationCompletion, OperationContext,
};

pub(super) async fn shutdown_tasks(tasks: &TaskRegistry, deadline: Duration) -> TaskShutdownReport {
    let started = Instant::now();
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Runtime,
        operation: DiagnosticOperation::SessionLifecycle,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    let report = tasks.shutdown(deadline).instrument(span.clone()).await;
    span.in_scope(|| {
        record_task_shutdown(
            report.completed_count,
            report.timed_out_count,
            report.join_error_count,
        );
        let completion = if report.timed_out_count > 0 {
            OperationCompletion::failed(
                DiagnosticDomain::Runtime,
                DiagnosticOperation::SessionLifecycle,
                DiagnosticRole::Local,
                DiagnosticErrorType::ShutdownTimeout,
                started.elapsed(),
            )
        } else if report.join_error_count > 0 {
            OperationCompletion::failed(
                DiagnosticDomain::Runtime,
                DiagnosticOperation::SessionLifecycle,
                DiagnosticRole::Local,
                DiagnosticErrorType::JoinFailed,
                started.elapsed(),
            )
        } else {
            OperationCompletion::succeeded(
                DiagnosticDomain::Runtime,
                DiagnosticOperation::SessionLifecycle,
                DiagnosticRole::Local,
                started.elapsed(),
            )
        };
        complete_operation(completion);
    });
    report
}
