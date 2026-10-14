//! 当前剪贴板按需拉取客户端；完整准备能力归 Application 所有。

use async_trait::async_trait;

use crate::ids::DeviceId;

/// Failure surface for serving a pull request (holding side).
/// Failure surface for issuing a pull request (requesting side).
#[derive(Debug, thiserror::Error)]
pub enum ActiveClipboardPullClientError {
    /// The peer could not be reached (no address, dial failure) or did not
    /// respond within the pull deadline.
    #[error("peer offline or pull timed out")]
    Unreachable,
    /// The peer responded but reported it does not hold the content (or its
    /// session was locked, so it could not serve).
    #[error("peer cannot serve the requested content")]
    NotAvailable,
    /// Stream / protocol I/O failure during the exchange.
    #[error("pull io")]
    Io(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Request the transfer envelope for `snapshot_hash` from a single peer.
///
/// Returns the transfer-encrypted envelope bytes the holder produced (the
/// same format Application 的完整拉取准备能力 returns), ready to be
/// decoded + stored through the standard inbound pipeline. The call is bounded
/// by an implementation-defined deadline; a peer that is unreachable or slow
/// surfaces as [`ActiveClipboardPullClientError::Unreachable`], and a peer that
/// answers "I don't hold it" surfaces as
/// [`ActiveClipboardPullClientError::NotAvailable`].
#[async_trait]
pub trait ActiveClipboardPullClientPort: Send + Sync {
    /// Pull the content behind `snapshot_hash` from `peer`.
    async fn pull(
        &self,
        peer: &DeviceId,
        snapshot_hash: &str,
    ) -> Result<Vec<u8>, ActiveClipboardPullClientError>;
}
