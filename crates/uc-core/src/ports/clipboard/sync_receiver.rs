//! 入站剪贴板负载与一次性结算回执，不携带观测信息。

use bytes::Bytes;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use super::sync_dispatch::ClipboardHeader;
use crate::ids::DeviceId;
use crate::ports::ConnectionChannel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundClipboardDisposition {
    Applied,
    Duplicate,
    Rejected,
}

#[derive(Clone)]
pub struct InboundClipboardReceipt {
    sender: Arc<Mutex<Option<oneshot::Sender<InboundClipboardDisposition>>>>,
}

impl std::fmt::Debug for InboundClipboardReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundClipboardReceipt")
            .finish_non_exhaustive()
    }
}

impl InboundClipboardReceipt {
    pub fn pending() -> (Self, InboundClipboardResult) {
        let (sender, receiver) = oneshot::channel();
        (
            Self {
                sender: Arc::new(Mutex::new(Some(sender))),
            },
            InboundClipboardResult { receiver },
        )
    }

    pub fn finish(&self, disposition: InboundClipboardDisposition) -> bool {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        sender.is_some_and(|sender| sender.send(disposition).is_ok())
    }
}

pub struct InboundClipboardResult {
    receiver: oneshot::Receiver<InboundClipboardDisposition>,
}

impl InboundClipboardResult {
    pub async fn wait(self) -> Option<InboundClipboardDisposition> {
        self.receiver.await.ok()
    }
}

impl std::future::IntoFuture for InboundClipboardResult {
    type Output = Option<InboundClipboardDisposition>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.receiver.await.ok() })
    }
}

/// One inbound clipboard delivery. Ciphertext is still sealed — decryption
/// and content-hash dedup happen in the application layer.
#[derive(Debug, Clone)]
pub struct InboundClipboard {
    pub peer_device_id: DeviceId,
    pub header: ClipboardHeader,
    pub ciphertext: Bytes,
    /// Connection path observed when the receiver accepted this delivery.
    pub transport: ConnectionChannel,
    pub receipt: InboundClipboardReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inbound_receipt_settles_once_with_the_first_application_result() {
        let (receipt, result) = InboundClipboardReceipt::pending();

        assert!(receipt.finish(InboundClipboardDisposition::Applied));
        assert!(!receipt.finish(InboundClipboardDisposition::Rejected));
        assert_eq!(result.await, Some(InboundClipboardDisposition::Applied));
    }
}
