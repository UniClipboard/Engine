//! 单轮网络响应期限由完整负责人给出；取消后仍等待同次实际工作结算。
use super::{BlobDownloadResources, BlobExchangeError, BlobReceived, Progress};
use crate::connectivity::StreamLease;
use crate::transfer::policy::RECOVERY_TIMEOUT;
use std::sync::{Arc, PoisonError};
use tokio_util::sync::CancellationToken;
use uc_core::ports::blob::BlobDigest;

impl BlobDownloadResources {
    pub(super) async fn receive_round(
        &self,
        stream: StreamLease,
        digest: BlobDigest,
        cancel: &CancellationToken,
        progress: Arc<Progress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        let logical_deadline = progress
            .timing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .deadline()
            .at;
        let round_cancel = cancel.child_token();
        let receiving = self.transport.exchange.receive(
            stream,
            logical_deadline,
            RECOVERY_TIMEOUT,
            digest,
            round_cancel.clone(),
            progress,
        );
        tokio::pin!(receiving);
        tokio::select! {
            biased;
            _ = self.transport.stop.cancelled() => {
                round_cancel.cancel();
                receiving.await
            }
            result = &mut receiving => result,
        }
    }
}
