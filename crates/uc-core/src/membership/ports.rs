use async_trait::async_trait;

use crate::ids::{DeviceId, SpaceId};
use crate::membership::error::MembershipInitializationError;

use super::error::{
    CurrentMembershipIdentityError, GroupUpdateDispatchError, MembershipError,
    MembershipHistoryExchangeError, RelationshipStateResetError, SpaceSecurityStateResetError,
};
use super::member::SpaceMember;
use super::membership_history::MembershipHistoryMessage;
use super::revocation::{
    GroupEpoch, GroupRevocationResult, GroupUpdateDeliveryStatus, KeyEpochError,
    PendingGroupUpdate, PreparedRevocationResolution, RevocationId, RevocationRecord,
    RevocationStage, SpaceKeyMaterial,
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

    /// 按到期时间读取投递任务；恢复机会只唤醒匹配设备，不能释放拒绝状态。
    async fn due_group_updates(
        &self,
        space_id: &SpaceId,
        now_ms: i64,
        online_peer: Option<DeviceId>,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;

    /// 记录发送失败，不更改安全事实或确认结果。
    async fn record_group_update_failures(
        &self,
        space_id: &SpaceId,
        failures: &[(String, GroupUpdateDispatchError)],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError>;

    async fn group_update_delivery_status(
        &self,
        space_id: &SpaceId,
    ) -> Result<GroupUpdateDeliveryStatus, KeyEpochError>;

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

    /// 结清收件人已不在保留名单中的 outbox 消息，返回结清数量。
    ///
    /// 与 `acknowledge_recipient` 走同一条完成路径：剩余消息全部确认时撤销随即完成。
    /// 撤销不在分发阶段时没有可结清的投递，返回 0。
    async fn settle_obsolete_revocation_recipients(
        &self,
        revocation_id: &RevocationId,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError>;
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
        Err(KeyEpochError::StateIssue(
            super::KeyEpochStateIssue::UnsupportedOperation,
        ))
    }

    async fn resume_group_revocations(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError>;

    async fn due_space_group_updates(
        &self,
        now_ms: i64,
        online_peer: Option<DeviceId>,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;

    async fn record_space_group_update_failures(
        &self,
        failures: &[(String, GroupUpdateDispatchError)],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError>;

    async fn space_group_update_delivery_status(
        &self,
    ) -> Result<GroupUpdateDeliveryStatus, KeyEpochError>;

    async fn acknowledge_space_group_update(
        &self,
        update_id: &str,
        now_ms: i64,
    ) -> Result<bool, KeyEpochError>;

    /// 结清收件人已不在保留名单中的待投递项，返回结清数量。
    ///
    /// 名单由成员历史导出，实现只做队列读写与名单匹配，不自行判断成员资格，
    /// 也不受投递退避影响：整个队列一次判定，而不只是本轮到期项。
    async fn settle_obsolete_space_group_updates(
        &self,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError>;
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

#[async_trait]
pub trait SpaceMembershipInitializerPort: Send + Sync {
    async fn initialize(&self) -> Result<(), MembershipInitializationError>;
}
