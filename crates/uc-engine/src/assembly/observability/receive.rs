//! 只装饰完整重试与本地输出能力；实际完成权留在原Application接收任务。
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio_util::bytes::Bytes;
use tracing::Instrument;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobReadStorePort, BlobReceived,
};
use uc_application::facade::{RetryEntryReceiveError, RetryEntryReceiveOutcome};
use uc_core::ports::blob::BlobDigest;
use uc_observability_contract::diagnostics::connectivity::{
    ClipboardReceiveFailure, ClipboardReceiveObservation, ClipboardRetryOutcome,
};
use uc_observability_contract::diagnostics::{
    complete_operation, independent_operation_span, operation_span, DiagnosticDomain,
    DiagnosticErrorType, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationCompletion, OperationContext,
};

pub(crate) async fn observe_receive_retry(
    action: impl Future<Output = Result<RetryEntryReceiveOutcome, RetryEntryReceiveError>>,
) -> Result<RetryEntryReceiveOutcome, RetryEntryReceiveError> {
    let domain = DiagnosticDomain::Clipboard;
    let operation = DiagnosticOperation::ClipboardRetryReceive;
    let role = DiagnosticRole::Local;
    let span = independent_operation_span(OperationContext {
        domain,
        operation,
        role,
        kind: DiagnosticSpanKind::Internal,
    });
    let started = Instant::now();
    let completion_span = span.clone();
    let observation = ClipboardReceiveObservation::with_retry_completion(move |outcome| {
        let duration = started.elapsed();
        let completion = match outcome {
            ClipboardRetryOutcome::Succeeded => {
                OperationCompletion::succeeded(domain, operation, role, duration)
            }
            ClipboardRetryOutcome::Partial => {
                OperationCompletion::partial(domain, operation, role, duration)
            }
            ClipboardRetryOutcome::Cancelled => {
                OperationCompletion::cancelled(domain, operation, role, duration)
            }
            ClipboardRetryOutcome::Deferred => {
                OperationCompletion::deferred(domain, operation, role, duration)
            }
            ClipboardRetryOutcome::Rejected => {
                OperationCompletion::rejected(domain, operation, role, duration)
            }
            ClipboardRetryOutcome::Skipped => {
                OperationCompletion::skipped(domain, operation, role, duration)
            }
            ClipboardRetryOutcome::Failed(failure) => OperationCompletion::failed(
                domain,
                operation,
                role,
                match failure {
                    ClipboardReceiveFailure::ApplyStorageFull
                    | ClipboardReceiveFailure::ApplyPermissionDenied
                    | ClipboardReceiveFailure::ApplyReadOnly
                    | ClipboardReceiveFailure::ApplyIoFailed => DiagnosticErrorType::Storage,
                    _ => DiagnosticErrorType::Internal,
                },
                duration,
            ),
        };
        completion_span.in_scope(|| complete_operation(completion));
    });
    let result = observation.scope(action.instrument(span)).await;
    // Accepted只交接完成权；真实任务结束后才结算业务记录。
    let immediate = match &result {
        Ok(RetryEntryReceiveOutcome::Accepted { .. }) => None,
        Ok(RetryEntryReceiveOutcome::AlreadyReceiving | RetryEntryReceiveOutcome::Superseded) => {
            Some(ClipboardRetryOutcome::Skipped)
        }
        Ok(
            RetryEntryReceiveOutcome::CancellationIncomplete
            | RetryEntryReceiveOutcome::Unavailable,
        ) => Some(ClipboardRetryOutcome::Deferred),
        Ok(RetryEntryReceiveOutcome::AccessDenied) | Err(RetryEntryReceiveError::InvalidInput) => {
            Some(ClipboardRetryOutcome::Rejected)
        }
        Err(RetryEntryReceiveError::Busy) => Some(ClipboardRetryOutcome::Deferred),
        // 下层来源仍由原返回值保留，诊断只使用固定类别。
        Err(_) => None,
    };
    if let Some(outcome) = immediate {
        observation.finish_retry(outcome);
    }
    result
}

pub(crate) fn observe_blob_exports(
    inner: Arc<dyn BlobReadStorePort>,
) -> Arc<dyn BlobReadStorePort> {
    Arc::new(ObservedBlobExports { inner })
}

struct ObservedBlobExports {
    inner: Arc<dyn BlobReadStorePort>,
}

#[async_trait]
impl BlobReadStorePort for ObservedBlobExports {
    async fn protect(&self, digest: BlobDigest) -> Result<Option<BlobReceived>, BlobExchangeError> {
        self.inner.protect(digest).await
    }

    async fn read(&self, digest: BlobDigest) -> Result<Bytes, BlobExchangeError> {
        self.inner.read(digest).await
    }

    async fn export(&self, digest: BlobDigest, target: &Path) -> Result<u64, BlobExchangeError> {
        let domain = DiagnosticDomain::Clipboard;
        let operation = DiagnosticOperation::ClipboardExportFile;
        let role = DiagnosticRole::Local;
        let span = operation_span(OperationContext {
            domain,
            operation,
            role,
            kind: DiagnosticSpanKind::Internal,
        });
        async {
            let started = Instant::now();
            let result = self.inner.export(digest, target).await;
            let completion = match &result {
                Ok(_) => OperationCompletion::succeeded(domain, operation, role, started.elapsed()),
                Err(error) => OperationCompletion::failed(
                    domain,
                    operation,
                    role,
                    if error.kind == BlobExchangeFailure::Store {
                        DiagnosticErrorType::Storage
                    } else {
                        DiagnosticErrorType::Internal
                    },
                    started.elapsed(),
                ),
            };
            complete_operation(completion);
            result
        }
        .instrument(span)
        .await
    }
}
