use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::Instrument;
use uc_core::FileTransferCancellationReason;
use uc_observability_contract::diagnostics::ObservationContext;

use crate::runtime_lifecycle::LifecycleError;

use super::session::ReceiverTransferHandleRegistry;
use super::FileTransferApplicationError;

pub(super) async fn close(
    registry: Arc<ReceiverTransferHandleRegistry>,
) -> Result<(), FileTransferApplicationError> {
    registry.close();
    cancel_active(registry, FileTransferCancellationReason::Unknown).await
}

pub(super) async fn cancel_active(
    registry: Arc<ReceiverTransferHandleRegistry>,
    reason: FileTransferCancellationReason,
) -> Result<(), FileTransferApplicationError> {
    let observation = ObservationContext::capture();
    tokio::spawn(
        observation.scope(
            async move {
                let _creation = registry.lock_creation().await;
                let sessions = registry.snapshot().await;
                let mut errors = Vec::new();
                let mut cancellations = JoinSet::new();
                for session in sessions {
                    let observation = ObservationContext::capture();
                    cancellations.spawn(
                        observation
                            .scope(async move { session.cancel(reason).await }.in_current_span()),
                    );
                }
                // 先通知全部传输，再统一等待并收集结果，单个慢收尾不阻止其他传输开始停止。
                while let Some(result) = cancellations.join_next().await {
                    match result {
                        Ok(Ok(_))
                        | Ok(Err(FileTransferApplicationError::TransferAlreadyFinished {
                            ..
                        })) => {}
                        Ok(Err(source)) => errors.push(source.into()),
                        Err(source) => errors.push(source.into()),
                    }
                }
                LifecycleError::from_errors(errors).map_err(FileTransferApplicationError::Cleanup)
            }
            .in_current_span(),
        ),
    )
    .await
    .map_err(|source| {
        FileTransferApplicationError::Cleanup(LifecycleError {
            primary: source.into(),
            additional: Vec::new(),
        })
    })?
}
