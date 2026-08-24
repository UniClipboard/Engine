use uc_core::membership::{MembershipEventId, WorkspaceSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitiateSpaceMemberRemovalResult {
    pub removal_event_id: MembershipEventId,
    pub snapshot: WorkspaceSnapshot,
}
