//! 只装饰用户明确发起的完整核对；普通读视图及协议后台工作不创建业务入口。
use std::future::Future;
use std::time::Instant;
use tracing::{Instrument, Span};
use uc_application::facade::{TransferProgressQueryError, TransferProgressReconciliation};
use uc_observability_contract::diagnostics::{
    independent_operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationContext, ProgressReconciliationOutcome,
};

pub(crate) async fn observe_progress_reconciliation(
    action: impl Future<Output = Result<TransferProgressReconciliation, TransferProgressQueryError>>,
) -> Result<TransferProgressReconciliation, TransferProgressQueryError> {
    let span = independent_operation_span(OperationContext {
        domain: DiagnosticDomain::Clipboard,
        operation: DiagnosticOperation::ClipboardReconcileProgress,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    let mut observation = ReconciliationObservation {
        span: span.clone(),
        started: Instant::now(),
        outcome: ProgressReconciliationOutcome::Cancelled,
    };
    let result = action.instrument(span).await;
    observation.outcome = classify(&result);
    drop(observation);
    result
}

struct ReconciliationObservation {
    span: Span,
    started: Instant,
    outcome: ProgressReconciliationOutcome,
}
impl Drop for ReconciliationObservation {
    fn drop(&mut self) {
        self.span
            .in_scope(|| self.outcome.complete(self.started.elapsed()));
    }
}

fn classify(
    result: &Result<TransferProgressReconciliation, TransferProgressQueryError>,
) -> ProgressReconciliationOutcome {
    match result {
        Ok(TransferProgressReconciliation::Running) => ProgressReconciliationOutcome::Running,
        Ok(TransferProgressReconciliation::Settled) => ProgressReconciliationOutcome::Settled,
        Ok(TransferProgressReconciliation::Superseded) => ProgressReconciliationOutcome::Superseded,
        Ok(TransferProgressReconciliation::NotRetained) => {
            ProgressReconciliationOutcome::NotRetained
        }
        Ok(TransferProgressReconciliation::Unsupported) => {
            ProgressReconciliationOutcome::Unsupported
        }
        Ok(TransferProgressReconciliation::Unreachable) => {
            ProgressReconciliationOutcome::Unreachable
        }
        Ok(TransferProgressReconciliation::Busy) => ProgressReconciliationOutcome::Busy,
        Err(TransferProgressQueryError::AccessDenied(_)) => {
            ProgressReconciliationOutcome::AccessDenied
        }
        Err(TransferProgressQueryError::InvalidInput(_)) => {
            ProgressReconciliationOutcome::InvalidInput
        }
        Err(TransferProgressQueryError::InvalidData(_)) => {
            ProgressReconciliationOutcome::InvalidData
        }
        Err(TransferProgressQueryError::Unavailable(_)) => {
            ProgressReconciliationOutcome::Unavailable
        }
    }
}
