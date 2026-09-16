#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CurrentSpaceIdentityError {
    #[error("current Space identity is unavailable")]
    Unavailable,

    #[error("current Space identity is inconsistent")]
    Inconsistent,
}
