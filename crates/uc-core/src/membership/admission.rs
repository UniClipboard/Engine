use async_trait::async_trait;
use thiserror::Error;

use crate::ids::DeviceId;

/// Authoritative authorization check for an already identity-resolved peer.
///
/// Implementations must fail closed when the active space protection cannot
/// prove that a peer belongs to the current protection group.
#[async_trait]
pub trait PeerAdmissionPort: Send + Sync {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError>;
}

#[derive(Debug, Error)]
pub enum PeerAdmissionError {
    #[error("peer admission state is unavailable")]
    Unavailable,

    #[error("peer admission state is invalid")]
    InvalidState,

    #[error("peer admission check failed: {0}")]
    Internal(String),
}
