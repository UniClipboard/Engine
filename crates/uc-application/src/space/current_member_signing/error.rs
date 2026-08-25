#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CurrentMemberSignatureError {
    #[error("current member signing state is unavailable")]
    Unavailable,
    #[error("current member signing state is invalid")]
    InvalidState,
    #[error("current member signing state could not be loaded: {0}")]
    Repository(String),
}
