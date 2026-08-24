#[derive(Debug, thiserror::Error)]
pub enum InitiateSpaceMemberRemovalError {
    #[error("space member removal is unavailable")]
    Unavailable,

    #[error("space membership state is corrupted")]
    Corrupt,

    #[error("the local member has already been removed")]
    LocalMemberRemoved,

    #[error("the target member does not exist in the current space")]
    TargetNotFound,

    #[error("the local member cannot remove itself")]
    SelfTarget,

    #[error("space member removal failed")]
    Failed,
}
