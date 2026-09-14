//! Peer reachability contract.
//!
//! Tracks whether each `SpaceMember` is currently reachable on the iroh
//! endpoint. The roster reads its snapshot; the application connection
//! coordinator owns periodic verification and retry scheduling.
//!
//! `ensure_reachable` is a single-target primitive; batching ("pre-connect
//! the whole roster") lives in the application layer so this port stays
//! minimal.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use crate::ids::DeviceId;

/// Reachability snapshot for one member.
///
/// Intentionally three-valued: `Unknown` distinguishes "never probed" from
/// "probed and confirmed offline". No `Connecting` / `Degraded` — Slice 2
/// has no consumer that could act on those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachabilityState {
    Online,
    Offline,
    Unknown,
}

/// Notification delivered on state change.
#[derive(Debug, Clone)]
pub struct PeerReachabilityChanged {
    pub device_id: DeviceId,
    pub state: ReachabilityState,
    pub at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerReachabilityError {
    /// No stored [`PeerAddressRecord`](crate::ports::peer_address::PeerAddressRecord)
    /// for this device — cannot dial. Application layer treats this as
    /// "member is offline" rather than a fatal error.
    #[error("no known address for device {0:?}")]
    NoAddress(DeviceId),
    #[error("internal peer reachability failure")]
    Internal {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl PeerReachabilityError {
    pub fn internal(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Internal {
            source: Box::new(source),
        }
    }
}

#[async_trait]
pub trait PeerReachabilityPort: Send + Sync {
    /// Reuse recent evidence or verify the target, dialing only when needed.
    ///
    /// Returns the resulting state — typically `Online` on success, `Offline`
    /// on dial failure. A `NoAddress` error surfaces when the peer address
    /// repository has no record for this device.
    async fn ensure_reachable(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError>;

    /// Obtain current authenticated reachability evidence. A failed new
    /// connection cannot invalidate another connection's fresh response.
    /// Implementations with cached evidence must override this method.
    async fn verify_reachable(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        self.ensure_reachable(device).await
    }

    /// Report one failed communication attempt and request a reachability
    /// check. This does not establish Offline, close an accepted connection,
    /// change authorization, or replay the failed operation.
    async fn report_communication_failure(&self, device: &DeviceId) {
        let _ = device;
    }

    /// Forget every cached reachability observation for a device that is no
    /// longer part of the active space. Implementations with connection or
    /// state caches must evict them without emitting a new peer_reachability event.
    async fn forget(&self, device: &DeviceId) {
        let _ = device;
    }

    /// Close every live peer_reachability connection when the local device leaves its
    /// active space. Implementations must also clear cached reachability so
    /// peers from the previous space cannot remain online.
    async fn disconnect_all(&self) {}

    /// Re-enable inbound peer_reachability after a space has been created, restored,
    /// or joined following a local leave.
    async fn activate(&self) {}

    /// Read the current cached state without dialing.
    ///
    /// Returns `Unknown` if the device has never been probed in the current
    /// process lifetime.
    async fn current_state(&self, device: &DeviceId) -> ReachabilityState;

    /// Multi-consumer subscription for state-change events.
    ///
    /// Each call returns a fresh receiver. Lagging receivers drop messages
    /// per `broadcast` contract — acceptable because the latest state can
    /// always be recovered via [`current_state`].
    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged>;
}
