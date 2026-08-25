#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandleSpaceAdmissionMessageError {
    #[error("space is locked")]
    Locked,
    #[error("space admission message is invalid")]
    Invalid,
    #[error("space admission message requires recovery")]
    RecoveryRequired,
    #[error("space admission state changed")]
    StateChanged,
    #[error("space admission is unavailable")]
    Unavailable,
}
