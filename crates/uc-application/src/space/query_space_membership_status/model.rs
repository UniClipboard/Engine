use uc_core::ids::DeviceId;
use uc_core::membership::MembershipEventId;
use uc_core::ports::ReachabilityState;

use crate::space::admission::{CurrentJoinStatus, PendingInboundMember};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMembership {
    Active,
    Removed,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRelationship {
    Consistent,
    PendingLocalDecision,
    Diverged,
    Unverifiable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCompatibility {
    Compatible,
    UpgradeRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRelationship {
    Usable,
    WaitingForLocalDecision,
    PausedGroupDiverged,
    PausedUpgradeRequired,
    PausedUnverifiable,
    RemovedLocalDevice,
    RemovedPeerDevice,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceMembershipChangeChoice {
    ApplyChange,
    KeepCurrentDeviceGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceMembershipAction {
    ApplyCurrentChange,
    KeepCurrentDeviceGroup,
    ConfirmApplyRemovesLocalDevice,
    RejoinDeviceGroup,
    UpdateThisDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionUnavailableReason {
    NoCurrentChange,
    ChangeNoLongerCurrent,
    LocalDeviceConfirmationRequired,
    LocalDeviceRemoved,
    RecoveryNotAvailableInThisVersion,
    PeerUpgradeRequired,
    DeviceFactsUnverifiable,
    EngineUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAvailability {
    NotAvailableInThisVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMembershipChangeImpact {
    pub usable_device_ids: Vec<DeviceId>,
    pub paused_device_ids: Vec<DeviceId>,
    pub local_device_outcome: DeviceMembership,
    pub requires_rejoin_device_ids: Vec<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSpaceMembershipChange {
    pub change_id: MembershipEventId,
    pub proposed_by_device_id: DeviceId,
    pub target_device_ids: Vec<DeviceId>,
    pub includes_local_device: bool,
    pub apply_impact: SpaceMembershipChangeImpact,
    pub keep_current_impact: SpaceMembershipChangeImpact,
    pub allowed_choices: Vec<SpaceMembershipChangeChoice>,
    pub blocked_reason: Option<ActionUnavailableReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMemberRelationship {
    pub device_id: DeviceId,
    pub display_name: String,
    pub is_local: bool,
    pub reachability: ReachabilityState,
    pub membership: DeviceMembership,
    pub group_relationship: GroupRelationship,
    pub compatibility: DeviceCompatibility,
    pub sync_relationship: SyncRelationship,
    pub available_actions: Vec<SpaceMembershipAction>,
    pub blocked_reason: Option<ActionUnavailableReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMembershipStatus {
    pub revision: u64,
    pub local_device_id: DeviceId,
    pub local_membership: DeviceMembership,
    pub current_change: Option<PendingSpaceMembershipChange>,
    pub current_join: Option<CurrentJoinStatus>,
    pub pending_inbound_member: Option<PendingInboundMember>,
    pub devices: Vec<SpaceMemberRelationship>,
    pub recovery: RecoveryAvailability,
    pub allowed_actions: Vec<SpaceMembershipAction>,
    pub blocked_reason: Option<ActionUnavailableReason>,
    pub updated_at_ms: i64,
}
