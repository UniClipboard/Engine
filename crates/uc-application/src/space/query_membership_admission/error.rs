#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueryMembershipAdmissionError {
    #[error("space is locked")]
    Locked,
    #[error("membership admission recovery is required")]
    RecoveryRequired,
    #[error("membership admission is unavailable")]
    Unavailable,
}
