use thiserror::Error;

use crate::ids::DeviceId;

/// Boundary error for the membership domain.
///
/// Infrastructure adapters map their internal failures (DB, I/O, etc.)
/// into `Repository` when crossing the port boundary. Use cases surface
/// `AlreadyAdmitted` and `NotFound` based on the business semantics they
/// enforce on top of the (thin) repository port.
#[derive(Debug, Error)]
pub enum MembershipError {
    #[error("member `{0}` has already been admitted")]
    AlreadyAdmitted(DeviceId),

    #[error("member `{0}` not found")]
    NotFound(DeviceId),

    #[error("membership repository failure: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipCandidateRepositoryError {
    #[error("membership candidate storage is locked")]
    Locked,
    #[error("membership candidate storage is corrupt")]
    Corrupt,
    #[error("membership candidate repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VerifiedPeerPromotionError {
    #[error("verified peer promotion storage is locked")]
    Locked,
    #[error("verified peer promotion storage is corrupt")]
    Corrupt,
    #[error("verified peer promotion failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipAnnouncementRepositoryError {
    #[error("membership announcement storage is locked")]
    Locked,
    #[error("membership announcement storage is corrupt")]
    Corrupt,
    #[error("membership announcement repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipOutboxRepositoryError {
    #[error("membership outbox storage is locked")]
    Locked,
    #[error("membership outbox storage is corrupt")]
    Corrupt,
    #[error("membership outbox repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdmissionAttemptRepositoryError {
    #[error("admission attempt storage is locked")]
    Locked,
    #[error("admission attempt storage is corrupt")]
    Corrupt,
    #[error("admission attempt already exists")]
    AlreadyExists,
    #[error("admission attempt was not found")]
    NotFound,
    #[error("admission attempt version conflicts with persisted state")]
    VersionConflict,
    #[error("admission attempt repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipSecurityUpdateError {
    #[error("membership security state is unavailable")]
    Unavailable,
    #[error("membership security update is invalid")]
    Invalid,
    #[error("membership security update failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipAppliedSecurityUpdateRepositoryError {
    #[error("membership applied update storage is locked")]
    Locked,
    #[error("membership applied update storage is corrupt")]
    Corrupt,
    #[error("membership applied update repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipGossipTransportError {
    #[error("membership gossip recipient is offline")]
    Offline,
    #[error("membership gossip was rejected")]
    Rejected,
    #[error("membership gossip protocol version is incompatible")]
    VersionIncompatible,
    #[error("membership gossip transport failed")]
    Transport,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipGossipEndpointError {
    #[error("membership gossip message was rejected")]
    Rejected,
    #[error("membership gossip message could not be persisted")]
    Persistence,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipAttestationError {
    #[error("membership peer is offline")]
    Offline,
    #[error("membership transport failed")]
    Transport,
    #[error("membership peer needs a security update")]
    MissingSecurityUpdate,
    #[error("membership protocol version is incompatible")]
    VersionIncompatible,
    #[error("membership proof was rejected")]
    Rejected,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipAttestationEndpointError {
    #[error("verified membership peer was rejected")]
    Rejected,
    #[error("membership peer is missing a security update")]
    MissingSecurityUpdate,
    #[error("verified membership peer could not be persisted")]
    Persistence,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkspaceConvergenceRepositoryError {
    #[error("workspace convergence storage is locked")]
    Locked,
    #[error("workspace convergence storage is corrupt")]
    Corrupt,
    #[error("workspace convergence repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CurrentMembershipIdentityError {
    #[error("current membership identity is unavailable")]
    Unavailable,
    #[error("current membership identity could not be loaded")]
    LoadFailed,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RelationshipStateResetError {
    #[error("relationship state reset failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceSecurityStateResetError {
    #[error("space security state reset failed: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CurrentMemberSignatureError {
    #[error("current member signing state is unavailable")]
    Unavailable,
    #[error("current member signing state is invalid")]
    InvalidState,
    #[error("current member signing state could not be loaded: {0}")]
    Repository(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GroupUpdateDispatchError {
    #[error("group update recipient is offline")]
    Offline,
    #[error("group update was rejected")]
    Rejected,
    #[error("group update transport failed")]
    Transport,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MembershipHistoryExchangeError {
    #[error("membership history recipient is offline")]
    Offline,
    #[error("membership history exchange was rejected")]
    Rejected,
    #[error("membership history exchange transport failed")]
    Transport,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPeerProbeError {
    #[error("legacy peer probe target is offline")]
    Offline,
    #[error("legacy peer probe was rejected")]
    Rejected,
    #[error("legacy peer probe transport failed")]
    Transport,
}
