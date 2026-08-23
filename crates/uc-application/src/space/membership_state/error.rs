#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SpaceMembershipStateRepositoryError {
    #[error("space membership state storage is locked")]
    Locked,
    #[error("space membership state is corrupt")]
    Corrupt,
    #[error("space membership state storage is unavailable")]
    Unavailable,
}
