use async_trait::async_trait;

use crate::ids::DeviceId;

use super::error::MembershipError;
use super::gossip::{
    DeviceAnnouncement, PendingMembershipBatch, RelayedSecurityUpdate, SpaceMembershipCandidate,
    VerifiedMembershipPeer,
};
use super::member::SpaceMember;
use super::removal_intent::{
    MemberInstanceId, RemovalCausalProof, RemovalIntentId, RemovalNotice, SignedRemovalIntent,
};
use super::revocation::{
    GroupEpoch, GroupRevocationResult, KeyEpochError, PendingGroupUpdate,
    PreparedRevocationResolution, RevocationId, RevocationRecord, RevocationStage,
    SpaceKeyMaterial,
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
pub enum MembershipAppliedSecurityUpdateRepositoryError {
    #[error("membership applied update storage is locked")]
    Locked,
    #[error("membership applied update storage is corrupt")]
    Corrupt,
    #[error("membership applied update repository failed: {0}")]
    Repository(String),
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

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum WorkspaceConvergenceRepositoryError {
    #[error("workspace convergence storage is locked")]
    Locked,
    #[error("workspace convergence storage is corrupt")]
    Corrupt,
    #[error("workspace convergence repository failed: {0}")]
    Repository(String),
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
pub enum SpaceSecurityStateResetError {
    #[error("space security state reset failed: {0}")]
    Repository(String),
}

#[async_trait]
pub trait SpaceSecurityStateResetPort: Send + Sync {
    async fn clear_space_security_state_except(
        &self,
        active_space_id: &SpaceId,
    ) -> Result<(), SpaceSecurityStateResetError>;
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

// ============================================================================
// 离线优先成员移除(ADR-015 / specs/015)
// ============================================================================

/// 普通成员通道上的意图交换消息。
///
/// 已验证移除意图及其有界确认经同一通道幂等交换;接收方按稳定标识去重。
/// 恢复资料交接已由受限恢复通道承担,不再经过普通成员通道。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalExchangeMessage {
    /// 一条已验证意图(含因果证明)。
    Intent(Box<SignedRemovalIntent>),
    /// 意图已验收(有界确认,不含业务内容)。
    IntentAck(RemovalIntentId),
}

/// 受限迟交入口的提交消息。
///
/// 已被移除的设备(或其转发者)只能提交意图和验证所需的有界因果证明,
/// 响应不携带当前成员、摘要、代次、密钥或内容。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalLateSubmission {
    Intent(Box<SignedRemovalIntent>),
}

/// 受限入口的有界接收结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalLateAcceptance {
    /// 意图已验收并持久化。
    Accepted { intent_id: RemovalIntentId },
    /// 意图已验收但此前已知(幂等)。
    AlreadyKnown { intent_id: RemovalIntentId },
    /// 拒绝提交。稳定失败类别,不包含业务内容。
    Rejected { reason: RemovalLateRejectionReason },
}

/// 受限入口的稳定拒绝类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalLateRejectionReason {
    Invalid,
    InvalidSpaceLineage,
    MissingCausalHistory,
    LimitExceeded,
    Unavailable,
}

/// 当前因果视图中的一个成员(设备与其成员实例)。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemovalViewMember {
    pub device_id: DeviceId,
    pub instance: MemberInstanceId,
    pub signing_public_key: Vec<u8>,
}

/// 当前因果视图快照:创建意图所需的成员实例集合与验证证明。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemovalViewSnapshot {
    pub epoch: u64,
    pub members: Vec<RemovalViewMember>,
    pub causal_proof: RemovalCausalProof,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalIntentVerificationError {
    #[error("removal intent space lineage is invalid")]
    InvalidSpaceLineage,
    #[error("removal intent causal proof is invalid")]
    InvalidProof,
    #[error("removal intent signature is invalid")]
    BadSignature,
    #[error("removal intent membership is invalid")]
    InvalidMembership,
    #[error("removal intent verification is unavailable")]
    Unavailable,
}

/// 移除意图的密码学验证:因果证明、视图成员与签名。
#[async_trait]
pub trait RemovalIntentVerificationPort: Send + Sync {
    /// 验证意图的完整合法性。失败时接收方不得持久化或应用该意图。
    async fn verify_intent(
        &self,
        intent: &SignedRemovalIntent,
    ) -> Result<(), RemovalIntentVerificationError>;
}

/// 本机成员移除对内容发送的最小限制查询。
///
/// 已保存的移除意图必须在任何新的内容发送前生效。发送流程只需要知道某个
/// 设备是否已经被本机移除，不能读取意图、成员视图、收敛状态或恢复资料。
#[async_trait]
pub trait RemovalTargetGatePort: Send + Sync {
    /// 返回 `true` 时，调用方不得再向该设备发送新的业务内容。
    /// 实现无法安全判断时必须返回 `true`，保持失败关闭。
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool;
}

/// 准入前由成员移除协调器给出的唯一决定。
///
/// 邀请创建和使用都必须读取这一结果，不能自行根据成员列表、在线状态或
/// 旧事件推断。`SupersededInvitation` 仅表示邀请早于当前移除事实，不泄露
/// 任何成员或收敛信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalAdmissionDecision {
    Allowed,
    AwaitingConvergence,
    RecoveryRequired,
    SupersededInvitation,
    Unavailable,
}

/// 成员移除与准入之间的窄边界。
///
/// `invitation_generation` 是邀请创建时取得的空间准入编号。新移除事实会推进
/// 编号，因此收敛前的旧邀请即使尚未过期也不能重新建立旧权限。
#[async_trait]
pub trait RemovalAdmissionGatePort: Send + Sync {
    async fn admission_decision(&self, invitation_generation: u64) -> RemovalAdmissionDecision;

    async fn invitation_generation(&self) -> Result<u64, RemovalAdmissionDecision>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalExchangeError {
    #[error("removal exchange recipient is offline")]
    Offline,
    #[error("removal exchange was rejected")]
    Rejected,
    #[error("removal exchange transport failed")]
    Transport,
}

/// 普通成员通道上的意图/恢复资料交换。
#[async_trait]
pub trait RemovalExchangePort: Send + Sync {
    async fn exchange(
        &self,
        recipient: &DeviceId,
        message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, RemovalExchangeError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalLateSubmissionTransportError {
    #[error("removal late submission recipient is offline")]
    Offline,
    #[error("removal late submission transport failed")]
    Transport,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalLateSubmissionError {
    #[error("removal late submission was rejected")]
    Rejected,
    #[error("removal late submission limit exceeded")]
    LimitExceeded,
    #[error("removal late submission could not be persisted")]
    Persistence,
    #[error("removal late submission endpoint is unavailable")]
    Unavailable,
}

/// 已被移除设备向当前成员迟交历史意图的受限发送端。
///
/// 该端口只能提交一条历史意图，并只返回有界接收结果；不提供任何读取当前
/// 空间状态的能力。
#[async_trait]
pub trait RemovalLateSubmissionPort: Send + Sync {
    async fn submit_late(
        &self,
        recipient: &DeviceId,
        submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionTransportError>;
}

/// 受限迟交入口:接收已被移除发起者的历史意图。
///
/// 只返回有界接收结果;提交者不能借该入口取得成员列表、在线状态、
/// 收敛摘要、安全代次、密钥或内容。
#[async_trait]
pub trait RemovalLateSubmissionEndpointPort: Send + Sync {
    async fn handle_late_submission(
        &self,
        submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionError>;
}

/// 移除通知的有界接收结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalNoticeAcceptance {
    /// 通知已验收并持久化。
    Accepted { intent_id: RemovalIntentId },
    /// 通知此前已验收(幂等)。
    AlreadyKnown { intent_id: RemovalIntentId },
    /// 拒绝通知。稳定失败类别,不包含业务内容。
    Rejected {
        reason: RemovalNoticeRejectionReason,
    },
}

/// 移除通知的稳定拒绝类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalNoticeRejectionReason {
    Invalid,
    SpaceMismatch,
    Unavailable,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalNoticeTransportError {
    #[error("removal notice recipient is offline")]
    Offline,
    #[error("removal notice transport failed")]
    Transport,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalNoticeError {
    #[error("removal notice was rejected")]
    Rejected,
    #[error("removal notice limit exceeded")]
    LimitExceeded,
    #[error("removal notice could not be persisted")]
    Persistence,
    #[error("removal notice endpoint is unavailable")]
    Unavailable,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalNoticeVerificationError {
    #[error("removal notice issuer is not a view member")]
    InvalidMembership,
    #[error("removal notice signature is invalid")]
    BadSignature,
    #[error("removal notice verification is unavailable")]
    Unavailable,
}

/// 当前成员向被移除设备定向投递移除通知的受限发送端。
///
/// 通知只携带“成员资格已终止”的稳定事实,不含成员列表、收敛摘要、
/// 安全代次、密钥或内容;接收方只返回有界结果。
#[async_trait]
pub trait RemovalNoticePort: Send + Sync {
    async fn send_notice(
        &self,
        recipient: &DeviceId,
        notice: RemovalNotice,
    ) -> Result<RemovalNoticeAcceptance, RemovalNoticeTransportError>;
}

/// 被移除设备接收移除通知的受限入口。
///
/// 接收方按本机已保存的因果视图公开签名资料核对签发者并验签;
/// 任一核对失败即拒绝且不改变状态(失败关闭)。
#[async_trait]
pub trait RemovalNoticeEndpointPort: Send + Sync {
    async fn handle_notice(
        &self,
        notice: RemovalNotice,
    ) -> Result<RemovalNoticeAcceptance, RemovalNoticeError>;
}

/// 移除通知的密码学验证:签发者签名与公钥匹配。
///
/// 签发者必须属于接收方保存的因果视图(成员资格核对由调用方完成,
/// 它持有视图公开签名资料);本端口只验证签名本身。
#[async_trait]
pub trait RemovalNoticeVerificationPort: Send + Sync {
    async fn verify_notice_signature(
        &self,
        notice: &RemovalNotice,
        issuer_public_key: &[u8],
    ) -> Result<(), RemovalNoticeVerificationError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RemovalRecoveryError {
    #[error("removal recovery state is unavailable")]
    Unavailable,
    #[error("removal recovery material is invalid")]
    InvalidMaterial,
    #[error("removal recovery is out of order")]
    OutOfOrder,
    #[error("removal recovery repository failed: {0}")]
    Repository(String),
}

/// 安全状态推进:意图集合决定有效成员集合,OpenMLS 只负责落实。
#[async_trait]
pub trait RemovalRecoveryPort: Send + Sync {
    /// 本机当前因果视图(成员实例集合与验证证明)。
    async fn current_view(&self) -> Result<RemovalViewSnapshot, RemovalRecoveryError>;

    /// 本机在空间中的成员实例。`None` 表示本机不在当前成员集合中。
    async fn own_instance(&self) -> Result<Option<MemberInstanceId>, RemovalRecoveryError>;
}

/// 移除交换的接收端(服务端 handler 侧)。
#[async_trait]
pub trait RemovalExchangeEndpointPort: Send + Sync {
    async fn handle_exchange(
        &self,
        source_device_id: &DeviceId,
        message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, RemovalExchangeError>;
}
