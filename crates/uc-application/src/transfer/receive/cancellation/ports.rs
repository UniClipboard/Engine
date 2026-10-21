use crate::transfer::blob::facade::{
    BlobTransferError, BlobTransferFacade, TransferCancellationWait,
};
use uc_core::file_transfer::FileTransferCancellationReason;

/// 与 [`BlobTransferFacade::cancel_inbound_attempt`] 同签名，供用例替换为测试替身。
#[async_trait::async_trait]
pub(crate) trait AttemptTransferCancellation: Send + Sync {
    async fn cancel_inbound_attempt(
        &self,
        attempt_id: &str,
        reason: FileTransferCancellationReason,
        wait: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError>;
}

#[async_trait::async_trait]
impl AttemptTransferCancellation for BlobTransferFacade {
    async fn cancel_inbound_attempt(
        &self,
        attempt_id: &str,
        reason: FileTransferCancellationReason,
        wait: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError> {
        BlobTransferFacade::cancel_inbound_attempt(self, attempt_id, reason, wait).await
    }
}
