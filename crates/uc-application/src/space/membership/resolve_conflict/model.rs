use uc_core::membership::{
    MembershipBranchId, MembershipBranchTransitionPhaseV1, MembershipConflictChoice,
    MembershipConflictId,
};

use crate::space::membership::{
    DeviceTrustMembership, DeviceTrustStatus, MembershipConflictMember, MembershipConflictStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGroupChoiceImpact {
    /// 选择后的候选同步范围，不授予实际发送权限。
    pub sync_scope_device_ids: Vec<uc_core::DeviceId>,
    pub paused_device_ids: Vec<uc_core::DeviceId>,
    pub pending_confirmation_device_ids: Vec<uc_core::DeviceId>,
    pub requires_rejoin_device_ids: Vec<uc_core::DeviceId>,
    pub local_membership: DeviceTrustMembership,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipConflictBranchView {
    pub branch_id: MembershipBranchId,
    pub is_local: bool,
    pub choice: MembershipConflictChoice,
    pub members: Option<Vec<MembershipConflictMember>>,
    pub impact: Option<DeviceGroupChoiceImpact>,
    pub source_device_ids: Vec<uc_core::DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipConflictView {
    pub conflict_id: MembershipConflictId,
    pub status: MembershipConflictStatus,
    pub selected_branch_id: Option<MembershipBranchId>,
    pub transition_phase: Option<MembershipBranchTransitionPhaseV1>,
    pub detected_at_revision: u64,
    pub evidence_peer_count: usize,
    pub branches: [MembershipConflictBranchView; 2],
    pub local_resolution_completed: bool,
    pub explanation: uc_core::membership::MembershipConflictExplanation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipConflictsView {
    pub revision: u64,
    pub conflicts: Vec<MembershipConflictView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveMembershipConflictInput {
    pub conflict_id: MembershipConflictId,
    pub target_branch_id: MembershipBranchId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveMembershipConflictResult {
    Completed {
        status: DeviceTrustStatus,
    },
    Pending {
        conflict_id: MembershipConflictId,
    },
    RePairingRequired {
        conflict_id: MembershipConflictId,
    },
    AlreadyCompleted {
        status: DeviceTrustStatus,
    },
    StateChanged {
        current_conflict_id: Option<MembershipConflictId>,
    },
}
