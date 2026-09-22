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
