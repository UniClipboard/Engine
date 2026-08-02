use async_trait::async_trait;

use crate::ids::DeviceId;

use super::error::MembershipError;
use super::gossip::PendingMembershipBatch;
use super::gossip::{DeviceAnnouncement, SpaceMembershipCandidate, VerifiedMembershipPeer};
use super::member::SpaceMember;
use super::revocation::{
    GroupEpoch, GroupRevocationResult, KeyEpochError, PendingGroupUpdate, RevocationId,
    RevocationRecord, RevocationStage, SpaceKeyMaterial,
};
use crate::ids::SpaceId;
use crate::ports::PeerAddressRecord;
use crate::security::IdentityFingerprint;
use crate::trusted_peer::TrustedPeer;

/// Persistence port for space members.
///
/// The port stays intentionally thin: admission and existence semantics
/// (e.g. how re-admitting a known device is handled, "cannot update a
/// missing member") are enforced by the use cases in the application
/// layer, not here.
#[async_trait]
pub trait MemberRepositoryPort: Send + Sync {
    /// Load a member by device id. Returns `None` when no record exists.
    async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError>;

    /// List every admitted member.
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError>;

    /// Create or replace a member record (upsert).
    async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError>;

    /// Remove a member record. Returns `true` when a record actually
    /// existed and was removed, `false` otherwise.
    async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipCandidateRepositoryError {
    #[error("membership candidate storage is locked")]
    Locked,
    #[error("membership candidate storage is corrupt")]
    Corrupt,
    #[error("membership candidate repository failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait MembershipCandidateRepositoryPort: Send + Sync {
    async fn get(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<Option<SpaceMembershipCandidate>, MembershipCandidateRepositoryError>;

    async fn list(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<SpaceMembershipCandidate>, MembershipCandidateRepositoryError>;

    async fn save(
        &self,
        candidate: &SpaceMembershipCandidate,
    ) -> Result<(), MembershipCandidateRepositoryError>;

    async fn remove(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<bool, MembershipCandidateRepositoryError>;

    async fn purge_expired(
        &self,
        space_id: &SpaceId,
        now_ms: i64,
    ) -> Result<usize, MembershipCandidateRepositoryError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VerifiedPeerPromotionError {
    #[error("verified peer promotion storage is locked")]
    Locked,
    #[error("verified peer promotion storage is corrupt")]
    Corrupt,
    #[error("verified peer promotion failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait VerifiedPeerPromotionPort: Send + Sync {
    async fn promote_verified_peer(
        &self,
        member: &SpaceMember,
        trusted_peer: &TrustedPeer,
        peer_address: &PeerAddressRecord,
        ready_candidate: &SpaceMembershipCandidate,
    ) -> Result<(), VerifiedPeerPromotionError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipAnnouncementRepositoryError {
    #[error("membership announcement storage is locked")]
    Locked,
    #[error("membership announcement storage is corrupt")]
    Corrupt,
    #[error("membership announcement repository failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait MembershipAnnouncementRepositoryPort: Send + Sync {
    async fn get(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<Option<DeviceAnnouncement>, MembershipAnnouncementRepositoryError>;

    async fn list(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<DeviceAnnouncement>, MembershipAnnouncementRepositoryError>;

    async fn save(
        &self,
        announcement: &DeviceAnnouncement,
    ) -> Result<(), MembershipAnnouncementRepositoryError>;

    async fn remove(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<bool, MembershipAnnouncementRepositoryError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipOutboxRepositoryError {
    #[error("membership outbox storage is locked")]
    Locked,
    #[error("membership outbox storage is corrupt")]
    Corrupt,
    #[error("membership outbox repository failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait MembershipOutboxRepositoryPort: Send + Sync {
    async fn get(
        &self,
        space_id: &SpaceId,
        recipient_device_id: &DeviceId,
        batch_id: &[u8; 32],
    ) -> Result<Option<PendingMembershipBatch>, MembershipOutboxRepositoryError>;

    async fn list_pending(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<PendingMembershipBatch>, MembershipOutboxRepositoryError>;

    async fn save(
        &self,
        pending: &PendingMembershipBatch,
    ) -> Result<(), MembershipOutboxRepositoryError>;

    async fn remove(
        &self,
        space_id: &SpaceId,
        recipient_device_id: &DeviceId,
        batch_id: &[u8; 32],
    ) -> Result<bool, MembershipOutboxRepositoryError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipSecurityState {
    pub space_id: SpaceId,
    pub group_epoch: u64,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipSecurityUpdateError {
    #[error("membership security state is unavailable")]
    Unavailable,
    #[error("membership security update is invalid")]
    Invalid,
    #[error("membership security update failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait MembershipSecurityUpdatePort: Send + Sync {
    async fn current_state(&self)
        -> Result<MembershipSecurityState, MembershipSecurityUpdateError>;

    async fn apply_group_epoch_update(
        &self,
        payload: &[u8],
    ) -> Result<u64, MembershipSecurityUpdateError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
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

#[async_trait]
pub trait MembershipGossipTransportPort: Send + Sync {
    async fn exchange(
        &self,
        recipient: &DeviceId,
        message: super::gossip::MembershipGossipMessage,
    ) -> Result<super::gossip::MembershipGossipMessage, MembershipGossipTransportError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipGossipEndpointError {
    #[error("membership gossip message was rejected")]
    Rejected,
    #[error("membership gossip message could not be persisted")]
    Persistence,
}

#[async_trait]
pub trait MembershipGossipEndpointPort: Send + Sync {
    async fn handle_message(
        &self,
        source_device_id: &DeviceId,
        message: super::gossip::MembershipGossipMessage,
    ) -> Result<super::gossip::MembershipGossipMessage, MembershipGossipEndpointError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
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

#[async_trait]
pub trait MembershipAttestationPort: Send + Sync {
    async fn attest_candidate(
        &self,
        candidate: &SpaceMembershipCandidate,
    ) -> Result<VerifiedMembershipPeer, MembershipAttestationError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipAttestationEndpointError {
    #[error("verified membership peer was rejected")]
    Rejected,
    #[error("membership peer is missing a security update")]
    MissingSecurityUpdate,
    #[error("verified membership peer could not be persisted")]
    Persistence,
}

#[async_trait]
pub trait MembershipAttestationEndpointPort: Send + Sync {
    async fn apply_relayed_security_updates(
        &self,
        space_id: &SpaceId,
        updates: &[super::gossip::RelayedSecurityUpdate],
    ) -> Result<u64, MembershipAttestationEndpointError>;

    async fn accept_verified_peer(
        &self,
        peer: VerifiedMembershipPeer,
    ) -> Result<(), MembershipAttestationEndpointError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentMembershipIdentity {
    pub space_id: SpaceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub identity_fingerprint: IdentityFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentMembershipAnnouncementMaterial {
    pub space_id: SpaceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub identity_fingerprint: IdentityFingerprint,
    pub transport_public_key: Vec<u8>,
    pub transport_address_blob: Vec<u8>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CurrentMembershipIdentityError {
    #[error("current membership identity is unavailable")]
    Unavailable,
    #[error("current membership identity could not be loaded")]
    LoadFailed,
}

#[async_trait]
pub trait CurrentMembershipIdentityPort: Send + Sync {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError>;
}

#[async_trait]
pub trait CurrentMembershipAnnouncementPort: Send + Sync {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError>;

    /// Wait until the transport-facing announcement material changes.
    /// Implementations must not emit the current value immediately.
    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RelationshipStateResetError {
    #[error("relationship state reset failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait RelationshipStateResetPort: Send + Sync {
    async fn clear_all_relationships(&self) -> Result<(), RelationshipStateResetError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CurrentMemberSignatureError {
    #[error("current member signing state is unavailable")]
    Unavailable,
    #[error("current member signing state is invalid")]
    InvalidState,
    #[error("current member signing state could not be loaded: {0}")]
    Repository(String),
}

#[async_trait]
pub trait CurrentMemberSignaturePort: Send + Sync {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError>;

    /// Sign `payload` using the local identity from the current active member set.
    async fn sign_current_member_payload(
        &self,
        payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError>;

    /// Verify that `signature` was produced by `member` over `payload` using
    /// the member's identity from the current active member set.
    async fn verify_current_member_payload(
        &self,
        member: &DeviceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginRevocationOutcome {
    Begun(RevocationRecord),
    Existing(RevocationRecord),
}

impl BeginRevocationOutcome {
    pub fn record(&self) -> &RevocationRecord {
        match self {
            Self::Begun(record) | Self::Existing(record) => record,
        }
    }
}

#[async_trait]
pub trait RevocationRepositoryPort: Send + Sync {
    async fn save_space_material(&self, material: &SpaceKeyMaterial) -> Result<(), KeyEpochError>;

    async fn load_space_material(
        &self,
        space_id: &SpaceId,
    ) -> Result<Option<SpaceKeyMaterial>, KeyEpochError>;

    async fn begin_revocation(
        &self,
        prepared: &RevocationRecord,
    ) -> Result<BeginRevocationOutcome, KeyEpochError>;

    async fn get_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<RevocationRecord>, KeyEpochError>;

    async fn list_incomplete_revocations(&self) -> Result<Vec<RevocationRecord>, KeyEpochError>;

    async fn stage_revocation(&self, stage: &RevocationStage) -> Result<(), KeyEpochError>;

    async fn load_staged_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<RevocationStage>, KeyEpochError>;

    async fn activate_revocation(
        &self,
        revocation_id: &RevocationId,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError>;

    async fn start_distribution(
        &self,
        revocation_id: &RevocationId,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError>;

    async fn acknowledge_recipient(
        &self,
        revocation_id: &RevocationId,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError>;
}

#[async_trait]
pub trait GroupRevocationPort: Send + Sync {
    async fn revoke_group_member(
        &self,
        target: &DeviceId,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError>;

    async fn acknowledge_group_update(
        &self,
        revocation_id: &RevocationId,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError>;

    async fn apply_group_epoch_update(&self, payload: &[u8]) -> Result<GroupEpoch, KeyEpochError>;

    async fn pending_group_updates(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;

    async fn query_group_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError>;

    async fn resume_group_revocations(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError>;

    async fn pending_space_group_updates(&self) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;

    async fn acknowledge_space_group_update(
        &self,
        update_id: &str,
        now_ms: i64,
    ) -> Result<bool, KeyEpochError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GroupUpdateDispatchError {
    #[error("group update recipient is offline")]
    Offline,
    #[error("group update was rejected")]
    Rejected,
    #[error("group update transport failed")]
    Transport,
}

#[async_trait]
pub trait GroupUpdateDispatchPort: Send + Sync {
    async fn dispatch_group_update(
        &self,
        update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError>;
}
