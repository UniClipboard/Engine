//! Clipboard receiver port (Slice 2 Phase 2).
//!
//! Complement to [`ClipboardDispatchPort`](super::sync_dispatch) — exposes
//! inbound payloads from peers on the clipboard ALPN as a broadcast event
//! stream. The application's `IngestInboundClipboardUseCase` subscribes
//! once at F1 `auto_start_network` completion and drives a background loop
//! that decrypts, dedupes and persists each arrival.
//!
//! `peer_device_id` is resolved by the adapter from the iroh connection's
//! remote endpoint id; unresolvable peers are rejected at the ALPN boundary
//! before reaching this stream.

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, oneshot};

use super::sync_dispatch::ClipboardHeader;
use crate::ids::DeviceId;

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
    pub receipt: InboundClipboardReceipt,
}

/// Multi-consumer subscription to the inbound clipboard event stream.
///
/// Lagging receivers drop messages per `broadcast` contract. That is
/// acceptable: the next content-hash comparison in the ingest use case
/// will still surface missed entries the next time the peer dispatches
/// them.
#[async_trait]
pub trait ClipboardReceiverPort: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<InboundClipboard>;
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
