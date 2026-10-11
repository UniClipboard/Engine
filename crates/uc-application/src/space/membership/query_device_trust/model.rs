use std::collections::BTreeMap;

use uc_core::ids::DeviceId;
use uc_core::membership::{MemberInstanceId, MembershipEventId};
use uc_core::ports::ReachabilityState;

use crate::space::admission::{CurrentJoinStatus, InboundPairing, PendingInboundMember};
use crate::space::membership::SpaceMemberPauseReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrustMembership {
    NoCurrentSpace,
    Active,
    Removed,
    PendingActivation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrustRelationship {
    Local,
    Consistent,
    ConfirmationPending,
    PendingLocalDecision,
    /// 设备已被本机历史移除，等待对方确认收到移除；这不是本机需要作出的决定。
    AwaitingRemovalAcknowledgement,
    Diverged,
    Invalid,
    UpgradeRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrustSyncState {
    Usable,
    Paused(SpaceMemberPauseReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingConfirmationStatus {
    AwaitingPeerConfirmation,
    Unconfirmed,
    Confirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PairingConfirmationTarget {
    pub member_instance_id: MemberInstanceId,
    pub add_event_id: MembershipEventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingConfirmationObservation {
    pub target: PairingConfirmationTarget,
    pub status: PairingConfirmationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDisplayStatus {
    pub current_join: Option<CurrentJoinStatus>,
    pub inbound_pairings: Vec<InboundPairing>,
    pub pending_inbound_member: Option<PendingInboundMember>,
    pub pairing_confirmations: Vec<PairingConfirmationObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceDeviceUpdatePhase {
    Updating,
    Completed,
    RetryableFailure,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceDeviceUpdateProblem {
    DeviceStateRejected,
    DeviceRelationshipConflict,
    DeviceSecurityUpdateRejected,
    DeviceUpgradeRequired,
    LocalIdentityMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceDeviceUpdateRecovery {
    ReviewDevices,
    UpdateApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceDeviceUpdateStatus {
    pub phase: SpaceDeviceUpdatePhase,
    pub reason: Option<SpaceDeviceUpdateProblem>,
    pub recovery: Option<SpaceDeviceUpdateRecovery>,
    pub next_retry_at_ms: Option<i64>,
}

impl SpaceDeviceUpdateStatus {
    pub const fn updating() -> Self {
        Self {
            phase: SpaceDeviceUpdatePhase::Updating,
            reason: None,
            recovery: None,
            next_retry_at_ms: None,
        }
    }

    pub const fn completed() -> Self {
        Self {
            phase: SpaceDeviceUpdatePhase::Completed,
            reason: None,
            recovery: None,
            next_retry_at_ms: None,
        }
    }

    pub const fn retryable_failure(next_retry_at_ms: i64) -> Self {
        Self {
            phase: SpaceDeviceUpdatePhase::RetryableFailure,
            reason: None,
            recovery: None,
            next_retry_at_ms: Some(next_retry_at_ms),
        }
    }

    pub const fn needs_attention(
        reason: SpaceDeviceUpdateProblem,
        recovery: SpaceDeviceUpdateRecovery,
    ) -> Self {
        Self {
            phase: SpaceDeviceUpdatePhase::NeedsAttention,
            reason: Some(reason),
            recovery: Some(recovery),
            next_retry_at_ms: None,
        }
    }

    pub const fn needs_attention_without_recovery(reason: SpaceDeviceUpdateProblem) -> Self {
        Self {
            phase: SpaceDeviceUpdatePhase::NeedsAttention,
            reason: Some(reason),
            recovery: None,
            next_retry_at_ms: None,
        }
    }
}

/// 某设备队首的未确认组密钥更新。没有该字段表示本机对它没有未确认的更新；
/// 它与成员保护状态（MLS 成员关系）是两件事：保护说明设备在组内，投递说明它是否已确认收到最新 epoch。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceGroupUpdate {
    pub epoch: u64,
    pub state: DeviceGroupUpdateState,
    pub attempts: u32,
    pub next_attempt_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceGroupUpdateState {
    /// 尚未送达（未尝试或设备不可达），按退避自动重试。
    Waiting,
    /// 设备在线但暂时无法应用，按退避自动重试。
    ReceiverUnavailable,
    /// 设备拒绝且原因未知（旧版设备或升级前遗留）；设备重新通过认证后自动重新评估。
    RejectedUnclassified,
    /// 设备明确拒绝；不会自动重发，需要核对该设备。
    RejectedByReceiver,
}

impl DeviceGroupUpdateState {
    pub const fn recovery(self) -> DeviceGroupUpdateRecovery {
        match self {
            Self::Waiting | Self::ReceiverUnavailable => DeviceGroupUpdateRecovery::Automatic,
            Self::RejectedUnclassified => DeviceGroupUpdateRecovery::AutomaticOnReconnect,
            Self::RejectedByReceiver => DeviceGroupUpdateRecovery::ReviewDevice,
        }
    }
}

/// Engine 自身是否会推进该更新；`Automatic*` 时宿主的“重新检查”不会改变任何结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceGroupUpdateRecovery {
    Automatic,
    AutomaticOnReconnect,
    ReviewDevice,
}

/// 组密钥投递给设备更新状态与逐设备投影提供的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityDeviceUpdates {
    pub status: SpaceDeviceUpdateStatus,
    pub devices: BTreeMap<DeviceId, DeviceGroupUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustObservation {
    pub device_id: DeviceId,
    pub display_name: Option<String>,
    pub reachability: ReachabilityState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustDevice {
    pub device_id: DeviceId,
    pub display_name: String,
    pub is_local: bool,
    pub reachability: ReachabilityState,
    pub membership: DeviceTrustMembership,
    pub relationship: DeviceTrustRelationship,
    pub sync_state: DeviceTrustSyncState,
    pub pairing_confirmation: Option<PairingConfirmationStatus>,
    pub group_update: Option<DeviceGroupUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustImpact {
    pub members: Vec<crate::space::membership::MembershipConflictMember>,
    pub member_device_ids: Vec<DeviceId>,
    pub usable_device_ids: Vec<DeviceId>,
    pub paused_device_ids: Vec<DeviceId>,
    pub local_membership: DeviceTrustMembership,
    pub requires_rejoin_device_ids: Vec<DeviceId>,
    pub pending_confirmation_device_ids: Vec<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDeviceTrustChange {
    pub change_id: MembershipEventId,
    pub proposed_by_device_id: DeviceId,
    pub target_device_ids: Vec<DeviceId>,
    pub includes_local_device: bool,
    pub apply_impact: DeviceTrustImpact,
    pub keep_current_impact: DeviceTrustImpact,
    pub explanation: uc_core::membership::MembershipConflictExplanation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustStatus {
    pub revision: u64,
    pub local_device_id: Option<DeviceId>,
    pub local_membership: DeviceTrustMembership,
    pub current_change: Option<PendingDeviceTrustChange>,
    pub current_join: Option<CurrentJoinStatus>,
    pub inbound_pairings: Vec<InboundPairing>,
    pub pending_inbound_member: Option<PendingInboundMember>,
    pub space_device_update: SpaceDeviceUpdateStatus,
    pub devices: Vec<DeviceTrustDevice>,
}

impl DeviceTrustStatus {
    pub(crate) fn no_current_space(revision: u64) -> Self {
        Self {
            revision,
            local_device_id: None,
            local_membership: DeviceTrustMembership::NoCurrentSpace,
            current_change: None,
            current_join: None,
            inbound_pairings: Vec::new(),
            pending_inbound_member: None,
            space_device_update: SpaceDeviceUpdateStatus::completed(),
            devices: Vec::new(),
        }
    }
}
