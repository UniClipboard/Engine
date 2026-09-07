use uc_core::ids::DeviceId;
use uc_core::membership::MembershipEventId;
use uc_core::ports::ReachabilityState;

use crate::space::admission::{CurrentJoinStatus, PendingInboundMember};
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
    pub pending_inbound_member: Option<PendingInboundMember>,
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
            pending_inbound_member: None,
            devices: Vec::new(),
        }
    }
}
