#[derive(Debug, thiserror::Error)]
pub enum DecidePendingMembershipRemovalError {
    #[error("pending membership removal is unavailable")]
    Unavailable,

    #[error("pending membership removal state is corrupted")]
    Corrupt,

    #[error("pending membership removal decision failed")]
    Failed,
}
