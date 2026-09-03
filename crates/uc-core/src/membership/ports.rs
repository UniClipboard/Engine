use async_trait::async_trait;

use crate::ids::{DeviceId, SpaceId};
use crate::membership::error::MembershipInitializationError;

use super::error::{
    CurrentMembershipIdentityError, GroupUpdateDispatchError, MembershipAttestationEndpointError,
    MembershipAttestationError, MembershipError, MembershipGossipEndpointError,
    MembershipGossipTransportError, MembershipHistoryExchangeError, MembershipSecurityUpdateError,
    RelationshipStateResetError, SpaceSecurityStateResetError,
};
use super::gossip::{SpaceMembershipCandidate, VerifiedMembershipPeer};
use super::member::SpaceMember;
use super::membership_history::MembershipHistoryMessage;
use super::revocation::{
    GroupEpoch, GroupRevocationResult, KeyEpochError, PendingGroupUpdate,
    PreparedRevocationResolution, RevocationId, RevocationRecord, RevocationStage,
    SpaceKeyMaterial,
};
use crate::security::IdentityFingerprint;

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

    async fn defer_space_group_update(
        &self,
        _update_id: &str,
        _now_ms: i64,
    ) -> Result<bool, KeyEpochError> {
        Ok(false)
    }
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

#[async_trait]
pub trait SpaceMembershipInitializerPort: Send + Sync {
    async fn initialize(&self) -> Result<(), MembershipInitializationError>;
}
