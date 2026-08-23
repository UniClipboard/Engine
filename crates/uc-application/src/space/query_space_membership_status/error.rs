#[derive(Debug, thiserror::Error)]
pub enum QuerySpaceMembershipStatusError {
    #[error("space membership status is unavailable")]
    Unavailable,
    #[error("space membership status is corrupted")]
    Corrupt,
    #[error("space membership status query failed")]
    Failed,
}
