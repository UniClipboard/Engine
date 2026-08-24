#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipHistoryRepositoryError {
    #[error("membership history storage is locked")]
    Locked,
    #[error("membership history is corrupt")]
    Corrupt,
    #[error("membership history changed concurrently")]
    Conflict,
    #[error("membership history storage is unavailable")]
    Unavailable,
}
