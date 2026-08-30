use uc_core::membership::{MembershipBranchId, MembershipConflictId};

use crate::space::membership::DeviceTrustStatus;

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
