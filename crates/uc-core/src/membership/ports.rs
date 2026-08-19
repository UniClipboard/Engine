use async_trait::async_trait;

use crate::ids::{DeviceId, SpaceId};

use super::admission_attempt::{
    AdmissionAttemptId, AdmissionAttemptV1, AdmissionInboxRecordV1, AdmissionOutboxMessageV1,
    AdmissionProfileMetadataV1, CurrentLocalJoinProjectionV1, TerminalAdmissionAttemptV1,
};
use super::error::{
    AdmissionAttemptRepositoryError, AdmissionOutboxDeliveryError,
    AdmissionSecurityTransitionError, AdmissionSpaceTransitionError, CurrentMemberSignatureError,
    CurrentMembershipIdentityError, GroupUpdateDispatchError,
    MembershipAnnouncementRepositoryError, MembershipAppliedSecurityUpdateRepositoryError,
    MembershipAttestationEndpointError, MembershipAttestationError,
    MembershipCandidateRepositoryError, MembershipError, MembershipGossipEndpointError,
    MembershipGossipTransportError, MembershipHistoryExchangeError,
    MembershipOutboxRepositoryError, MembershipSecurityUpdateError, RelationshipStateResetError,
    SpaceSecurityStateResetError, VerifiedPeerPromotionError, WorkspaceConvergenceRepositoryError,
};
use super::gossip::{
    DeviceAnnouncement, PendingMembershipBatch, RelayedSecurityUpdate, SpaceMembershipCandidate,
    VerifiedMembershipPeer,
};
use super::member::SpaceMember;
use super::member_instance::MemberInstanceId;
use super::membership_history::MembershipHistoryMessage;
use super::revocation::{
    GroupEpoch, GroupRevocationResult, KeyEpochError, PendingGroupUpdate,
    PreparedRevocationResolution, RevocationId, RevocationRecord, RevocationStage,
    SpaceKeyMaterial,
};
use super::versioned_membership_history::{
    AdmissionSecurityCommitmentV1, BaseMembershipHistoryPositionV1, MembershipCredential,
    MembershipCredentialId,
};
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

#[async_trait]
pub trait MembershipSecurityUpdatePort: Send + Sync {
    async fn current_state(&self)
        -> Result<MembershipSecurityState, MembershipSecurityUpdateError>;

    async fn apply_group_epoch_update(
        &self,
        payload: &[u8],
    ) -> Result<u64, MembershipSecurityUpdateError>;
}

/// Persistence for security updates this device has already applied.
///
/// Applied updates stay queryable so the device can relay them to peers
/// that are still behind on the group epoch, even when the device itself
/// was never a sponsor-seed candidate. `save` is idempotent per space and
/// update digest.
#[async_trait]
pub trait MembershipAppliedSecurityUpdateRepositoryPort: Send + Sync {
    async fn list(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<RelayedSecurityUpdate>, MembershipAppliedSecurityUpdateRepositoryError>;

    async fn save(
        &self,
        space_id: &SpaceId,
        update: &RelayedSecurityUpdate,
    ) -> Result<(), MembershipAppliedSecurityUpdateRepositoryError>;
}

#[async_trait]
pub trait MembershipGossipTransportPort: Send + Sync {
    async fn exchange(
        &self,
        recipient: &DeviceId,
        message: super::gossip::MembershipGossipMessage,
    ) -> Result<super::gossip::MembershipGossipMessage, MembershipGossipTransportError>;
}

#[async_trait]
pub trait MembershipGossipEndpointPort: Send + Sync {
    async fn handle_message(
        &self,
        source_device_id: &DeviceId,
        message: super::gossip::MembershipGossipMessage,
    ) -> Result<super::gossip::MembershipGossipMessage, MembershipGossipEndpointError>;
}

#[async_trait]
pub trait MembershipAttestationPort: Send + Sync {
    async fn attest_candidate(
        &self,
        candidate: &SpaceMembershipCandidate,
    ) -> Result<VerifiedMembershipPeer, MembershipAttestationError>;
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

/// 工作空间收敛状态的加密持久化。
///
/// 保存语义:工作空间变化、确认、待交接记录与本机安全效果必须在同一提交
/// 点保存;加载后必须校验状态属于请求的空间,不允许跨空间复用。任何已保存
/// 变化在崩溃恢复后必须仍然可见且顺序不变。
#[async_trait]
pub trait WorkspaceConvergenceRepositoryPort: Send + Sync {
    /// 保存完整收敛状态(变化链、确认、待交接记录、等待成员、阶段)。
    async fn save_state(
        &self,
        state: &super::workspace_convergence::WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError>;

    /// 加载当前空间的收敛状态。
    async fn load_state(
        &self,
    ) -> Result<
        Option<super::workspace_convergence::WorkspaceConvergenceState>,
        WorkspaceConvergenceRepositoryError,
    >;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalJoinStartMutationV1 {
    Create {
        replacement: AdmissionAttemptV1,
    },
    Supersede {
        expected_previous_attempt_id: AdmissionAttemptId,
        expected_previous_record_version: u64,
        previous_terminal: AdmissionAttemptV1,
        replacement: AdmissionAttemptV1,
    },
}

#[async_trait]
pub trait AdmissionAttemptRepositoryPort: Send + Sync {
    async fn commit_local_join_start(
        &self,
        _mutation: LocalJoinStartMutationV1,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "local join start storage is unavailable".to_owned(),
        ))
    }

    async fn create(
        &self,
        attempt: &AdmissionAttemptV1,
        consumed_invitation_digest: Option<[u8; 32]>,
        initial_membership_history_v2: Option<&[u8]>,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn load(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<AdmissionAttemptV1>, AdmissionAttemptRepositoryError>;

    async fn save_completion_recovery_challenge(
        &self,
        _attempt_id: AdmissionAttemptId,
        _challenge: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "completion recovery challenge storage is unavailable".to_owned(),
        ))
    }

    async fn load_completion_recovery_challenge(
        &self,
        _attempt_id: AdmissionAttemptId,
    ) -> Result<Option<Vec<u8>>, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "completion recovery challenge storage is unavailable".to_owned(),
        ))
    }

    async fn create_completion_helper(
        &self,
        _attempt: &AdmissionAttemptV1,
        _expected_challenge: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        Err(AdmissionAttemptRepositoryError::Repository(
            "completion helper storage is unavailable".to_owned(),
        ))
    }

    async fn compare_and_advance(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
        next: &AdmissionAttemptV1,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn compare_and_advance_with_membership_history_v2(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
        next: &AdmissionAttemptV1,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn compare_and_replace_membership_history_v2(
        &self,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn load_membership_history_v2(
        &self,
    ) -> Result<Option<Vec<u8>>, AdmissionAttemptRepositoryError>;

    async fn scan_recoverable(
        &self,
    ) -> Result<Vec<AdmissionAttemptV1>, AdmissionAttemptRepositoryError>;

    async fn compact_terminal(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
    ) -> Result<TerminalAdmissionAttemptV1, AdmissionAttemptRepositoryError>;

    async fn load_terminal(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<TerminalAdmissionAttemptV1>, AdmissionAttemptRepositoryError>;

    async fn profile_metadata(
        &self,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;

    async fn project_current_local_join(
        &self,
    ) -> Result<Option<CurrentLocalJoinProjectionV1>, AdmissionAttemptRepositoryError>;

    async fn advance_projection_floor(
        &self,
        expected_device_trust_revision: u64,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitationConsumeDeliveryResultV1 {
    Consumed,
    NotFound,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutboxDeliveryResultV1 {
    Deferred,
    Persisted(AdmissionInboxRecordV1),
    InvitationConsume(InvitationConsumeDeliveryResultV1),
    Rejected(AdmissionOutboxMessageV1),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutboxDeliveryRouteV1 {
    Invitation(Vec<u8>),
    Continuation(Vec<u8>),
}

#[async_trait]
pub trait AdmissionOutboxDeliveryPort: Send + Sync {
    async fn deliver(
        &self,
        attempt_id: AdmissionAttemptId,
        message: &AdmissionOutboxMessageV1,
        route: Option<&AdmissionOutboxDeliveryRouteV1>,
    ) -> Result<AdmissionOutboxDeliveryResultV1, AdmissionOutboxDeliveryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionCompletionRecoveryTransportError {
    Offline,
    Transport,
    Rejected,
}

#[async_trait]
pub trait AdmissionCompletionRecoveryPort: Send + Sync {
    async fn request_completion_recovery_challenge(
        &self,
        helper: &DeviceId,
        route: &[u8],
        hello: super::AdmissionCompletionRecoveryHelloV1,
        joiner_last_message_id: [u8; 32],
    ) -> Result<
        super::AdmissionCompletionRecoveryChallengeV1,
        AdmissionCompletionRecoveryTransportError,
    >;

    async fn submit_completion_recovery_response(
        &self,
        helper: &DeviceId,
        route: &[u8],
        hello: super::AdmissionCompletionRecoveryHelloV1,
        response: super::AdmissionCompletionRecoveryResponseV1,
    ) -> Result<crate::pairing::DurableAdmissionFrame, AdmissionCompletionRecoveryTransportError>;
}

#[async_trait]
pub trait AdmissionCompletionRecoveryEndpointPort: Send + Sync {
    async fn handle_completion_recovery_hello(
        &self,
        hello: super::AdmissionCompletionRecoveryHelloV1,
        transport_binding: super::AdmissionCompletionRecoveryTransportBindingV1,
        joiner_last_message_id: [u8; 32],
        helper_last_message_id: [u8; 32],
    ) -> Result<
        super::AdmissionCompletionRecoveryChallengeV1,
        AdmissionCompletionRecoveryTransportError,
    >;

    async fn handle_completion_recovery_response(
        &self,
        hello: super::AdmissionCompletionRecoveryHelloV1,
        response: super::AdmissionCompletionRecoveryResponseV1,
        transport_binding: super::AdmissionCompletionRecoveryTransportBindingV1,
    ) -> Result<crate::pairing::DurableAdmissionFrame, AdmissionCompletionRecoveryTransportError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionSpaceTransitionPreparationV2 {
    pub attempt_id: AdmissionAttemptId,
    pub target_space_id: String,
    pub target_security_commitment: super::AdmissionSecurityCommitmentV1,
    pub target_membership_history: Vec<u8>,
    pub target_security_state: Vec<u8>,
    pub target_protection_group_id: String,
    pub target_key_catalog: Vec<u8>,
    pub local_device_id: DeviceId,
    pub target_relationships: Vec<super::AdmissionChangeFacts>,
    pub relayed_group_updates: Vec<super::PendingGroupUpdate>,
    pub target_access_state: Vec<u8>,
    pub preserve_unreadable_history: bool,
}

impl std::fmt::Debug for AdmissionSpaceTransitionPreparationV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionSpaceTransitionPreparationV2")
            .field("attempt_id", &self.attempt_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionSpaceTransitionStepV2 {
    Advanced(super::AdmissionSpaceTransitionV2),
    Finished(super::AdmissionSpaceTransitionResultV2),
}

#[async_trait]
pub trait AdmissionSpaceTransitionPort: Send + Sync {
    async fn preflight_source_history(
        &self,
        _preserve_unreadable_history: bool,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Ok(())
    }

    async fn prepare_if_needed(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<super::AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError>;

    async fn advance(
        &self,
        transition: &super::AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError>;

    async fn discard_pre_activation(
        &self,
        transition: &super::AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionSecurityTransitionInput {
    pub attempt_id: [u8; 32],
    pub base_history_position: BaseMembershipHistoryPositionV1,
    pub candidate_core_digest: [u8; 32],
    pub key_catalog_digest: [u8; 32],
    pub admission_bundle_digest: [u8; 32],
}

impl std::fmt::Debug for AdmissionSecurityTransitionInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionSecurityTransitionInput([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorPreparedSecurityTransition {
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for SponsorPreparedSecurityTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorPreparedSecurityTransition")
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .field("welcome_len", &self.welcome.len())
            .field("public_commitment", &self.public_commitment)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct JoinerStagedSecurityTransition {
    pub staged_state: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for JoinerStagedSecurityTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JoinerStagedSecurityTransition")
            .field("staged_state", &"[REDACTED]")
            .field("public_commitment", &self.public_commitment)
            .finish()
    }
}

pub trait AdmissionSecurityTransitionPort: Send + Sync {
    fn prepare_sponsor(
        &self,
        sponsor_state: &[u8],
        candidate_identity: &[u8],
        key_package: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<SponsorPreparedSecurityTransition, AdmissionSecurityTransitionError>;

    fn stage_joiner(
        &self,
        pending_state: &[u8],
        key_package: &[u8],
        expected_space_id: &[u8],
        welcome: &[u8],
        commit: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<JoinerStagedSecurityTransition, AdmissionSecurityTransitionError>;

    fn derive_public_commitment(
        &self,
        staged_state: &[u8],
        commit: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionError>;

    fn activate(
        &self,
        staged_state: Vec<u8>,
        commit: &[u8],
        expected: &AdmissionSecurityCommitmentV1,
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<Vec<u8>, AdmissionSecurityTransitionError>;

    fn discard(&self, staged_state: Vec<u8>);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorAdmissionSecurityRecipient {
    pub device_id: DeviceId,
    pub credential_id: MembershipCredentialId,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorAdmissionSecurityRequest {
    pub space_id: SpaceId,
    pub attempt_id: [u8; 32],
    pub base_history_position: BaseMembershipHistoryPositionV1,
    pub candidate_core_digest: [u8; 32],
    pub candidate_identity: Vec<u8>,
    pub candidate_key_package: Vec<u8>,
    pub existing_recipients: Vec<SponsorAdmissionSecurityRecipient>,
}

impl std::fmt::Debug for SponsorAdmissionSecurityRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorAdmissionSecurityRequest")
            .field("space_id", &self.space_id)
            .field("recipient_count", &self.existing_recipients.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SponsorAdmissionSecurityDelivery {
    pub recipient: DeviceId,
    pub credential_id: MembershipCredentialId,
    pub payload: Vec<u8>,
}

impl std::fmt::Debug for SponsorAdmissionSecurityDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorAdmissionSecurityDelivery")
            .field("recipient", &self.recipient)
            .field("payload_len", &self.payload.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorPreparedAdmissionSecurity {
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
    pub target_protection_group_id: String,
    pub target_key_catalog: super::AdmissionContentKeyCatalogV1,
    pub existing_member_deliveries: Vec<SponsorAdmissionSecurityDelivery>,
}

impl std::fmt::Debug for SponsorPreparedAdmissionSecurity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorPreparedAdmissionSecurity")
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .field("welcome_len", &self.welcome.len())
            .field("public_commitment", &self.public_commitment)
            .field("target_key_catalog", &self.target_key_catalog)
            .field("delivery_count", &self.existing_member_deliveries.len())
            .finish()
    }
}

#[async_trait]
pub trait PrepareSponsorAdmissionSecurityPort: Send + Sync {
    async fn prepare_sponsor_admission_security(
        &self,
        request: SponsorAdmissionSecurityRequest,
    ) -> Result<SponsorPreparedAdmissionSecurity, AdmissionSecurityTransitionError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct ActivateSponsorAdmissionSecurityRequest {
    pub space_id: SpaceId,
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub expected_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for ActivateSponsorAdmissionSecurityRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivateSponsorAdmissionSecurityRequest")
            .field("space_id", &self.space_id)
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait ActivateSponsorAdmissionSecurityPort: Send + Sync {
    async fn activate_sponsor_admission_security(
        &self,
        request: ActivateSponsorAdmissionSecurityRequest,
    ) -> Result<(), AdmissionSecurityTransitionError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct ActivateCompletionHelperAdmissionSecurityRequest {
    pub space_id: SpaceId,
    pub attempt_id: [u8; 32],
    pub helper_device_id: DeviceId,
    pub helper_credential_id: MembershipCredentialId,
    pub candidate_core_digest: [u8; 32],
    pub security_commit: Vec<u8>,
    pub security_welcome: Vec<u8>,
    pub target_key_catalog: Vec<u8>,
    pub existing_member_deliveries: Vec<SponsorAdmissionSecurityDelivery>,
    pub expected_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for ActivateCompletionHelperAdmissionSecurityRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivateCompletionHelperAdmissionSecurityRequest")
            .field("space_id", &self.space_id)
            .field("delivery_count", &self.existing_member_deliveries.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait ActivateCompletionHelperAdmissionSecurityPort: Send + Sync {
    async fn activate_completion_helper_admission_security(
        &self,
        request: ActivateCompletionHelperAdmissionSecurityRequest,
    ) -> Result<(), AdmissionSecurityTransitionError>;
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

#[async_trait]
pub trait RelationshipStateResetPort: Send + Sync {
    async fn clear_all_relationships(&self) -> Result<(), RelationshipStateResetError>;
}

#[async_trait]
pub trait SpaceSecurityStateResetPort: Send + Sync {
    async fn clear_space_security_state_except(
        &self,
        active_space_id: &SpaceId,
    ) -> Result<(), SpaceSecurityStateResetError>;
}

#[async_trait]
pub trait CurrentMemberSignaturePort: Send + Sync {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError>;

    /// Historical-verification credential for the exact active local member.
    async fn current_membership_credential(
        &self,
        _device_id: &DeviceId,
    ) -> Result<MembershipCredential, CurrentMemberSignatureError> {
        Err(CurrentMemberSignatureError::Unavailable)
    }

    /// Stable local member instance derived from the active signing identity.
    async fn current_member_instance(
        &self,
        device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError>;

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

    /// Verify a payload against one exact member instance when the same
    /// stable device has more than one credential in the current group.
    async fn verify_member_instance_payload(
        &self,
        member: &DeviceId,
        _member_instance: MemberInstanceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        self.verify_current_member_payload(member, payload, signature)
            .await
    }
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

    async fn resolve_prepared_revocation(
        &self,
        revocation_id: &RevocationId,
        resolution: PreparedRevocationResolution,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError>;

    async fn commit_revocation_recovery(
        &self,
        stage: &RevocationStage,
        material: &SpaceKeyMaterial,
    ) -> Result<RevocationRecord, KeyEpochError>;

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

    async fn current_group_revocation(
        &self,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        Ok(None)
    }

    async fn continue_group_revocation(
        &self,
        _revocation_id: &RevocationId,
        _permanently_lost_device_ids: &[DeviceId],
        _now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        Err(KeyEpochError::Repository(
            "member revocation recovery unavailable".into(),
        ))
    }

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

#[async_trait]
pub trait GroupUpdateDispatchPort: Send + Sync {
    async fn dispatch_group_update(
        &self,
        update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError>;
}

// ============================================================================
// 成员历史核对（ADR-020）
// ============================================================================

/// 已认证成员之间唯一的成员核对传输边界。
///
/// 消息只携带有界签名历史、决定或确认，不提供旧移除意图、通知或迟交通道。
#[async_trait]
pub trait MembershipHistoryExchangePort: Send + Sync {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError>;
}

#[async_trait]
pub trait MembershipHistoryExchangeEndpointPort: Send + Sync {
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError>;
}

/// 成员历史对内容发送的最小限制查询。
///
/// 待本机决定、分叉或无效的对端不得接收新的业务内容。发送流程只需要知道某个
/// 设备是否已被本机阻断，不能读取成员历史或收敛状态。
#[async_trait]
pub trait ContentExchangeGatePort: Send + Sync {
    /// 返回 `true` 时，调用方不得再向该设备发送新的业务内容。
    /// 实现无法安全判断时必须返回 `true`，保持失败关闭。
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentWorkspacePeerScopeSource {
    CurrentHistory,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentWorkspaceLocalMembership {
    Active,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentWorkspacePeerSnapshot {
    pub revision: u64,
    pub source: CurrentWorkspacePeerScopeSource,
    pub local_membership: CurrentWorkspaceLocalMembership,
    pub peer_device_ids: Vec<DeviceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentWorkspacePeerScopeError {
    Locked,
    Unavailable,
    Corrupt,
}

#[async_trait]
pub trait CurrentWorkspacePeerScopePort: Send + Sync {
    async fn snapshot(
        &self,
    ) -> Result<CurrentWorkspacePeerSnapshot, CurrentWorkspacePeerScopeError>;
}

/// 准入前由成员历史负责人给出的唯一决定。
///
/// 邀请创建和使用都必须读取这一结果，不能自行根据成员列表、在线状态或
/// 旧状态推断。`SupersededInvitation` 仅表示邀请早于当前成员历史，不泄露
/// 任何成员或收敛信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipAdmissionDecision {
    Allowed,
    AwaitingConvergence,
    RecoveryRequired,
    SupersededInvitation,
    Unavailable,
}

/// 成员历史与准入之间的窄边界。
///
/// `invitation_generation` 是邀请创建时取得的空间准入编号。新成员历史会推进
/// 编号，因此旧邀请即使尚未过期也不能重新建立旧权限。
#[async_trait]
pub trait MembershipAdmissionGatePort: Send + Sync {
    async fn admission_decision(&self, invitation_generation: u64) -> MembershipAdmissionDecision;

    async fn invitation_generation(&self) -> Result<u64, MembershipAdmissionDecision>;
}
