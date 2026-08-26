use uc_core::membership::MembershipEventId;

#[derive(Debug, thiserror::Error)]
pub enum RemoveSpaceMemberError {
    #[error("space is locked")]
    Locked,
    #[error("space membership recovery is required")]
    RecoveryRequired,
    #[error("the local device is not an active member")]
    LocalMemberRemoved,
    #[error("the target device is not an active member")]
    TargetNotFound,
    #[error("the local device cannot remove itself")]
    SelfTarget,
    #[error("space membership changed")]
    StateChanged,
    #[error("space member removal is unavailable")]
    Unavailable,
    #[error("member removal {change_id} was committed but follow-up is pending")]
    CommittedButPending { change_id: MembershipEventId },
}
