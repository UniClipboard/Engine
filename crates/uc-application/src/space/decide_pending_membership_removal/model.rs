use uc_core::membership::{MembershipEventId, RemovalDecision};

use crate::space::query_space_membership_status::SpaceMembershipStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecidePendingMembershipRemovalResult {
    Accepted {
        removal_event_id: MembershipEventId,
        status: SpaceMembershipStatus,
    },
    Rejected {
        removal_event_id: MembershipEventId,
        status: SpaceMembershipStatus,
    },
    AlreadyDecided {
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
        status: SpaceMembershipStatus,
    },
    PendingRemovalChanged {
        current_removal_event_id: Option<MembershipEventId>,
        status: SpaceMembershipStatus,
    },
    SelfRemovalConfirmationRequired {
        removal_event_id: MembershipEventId,
        status: SpaceMembershipStatus,
    },
}
