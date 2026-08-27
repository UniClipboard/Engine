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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadMemberAdmissionError {
    #[error("space is locked")]
    Locked,
    #[error("space admission requires recovery")]
    RecoveryRequired,
    #[error("space admission is unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AcceptAdmissionError {
    #[error("space admission is locked")]
    Locked,
    #[error("space admission state changed")]
    StateChanged,
    #[error("space admission requires recovery")]
    RecoveryRequired,
    #[error("space admission is unavailable")]
    Unavailable,
}
