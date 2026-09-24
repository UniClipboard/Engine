#[derive(Debug, thiserror::Error)]
pub enum CurrentMemberSignatureError {
    #[error("current member signing state is unavailable")]
    Unavailable,
    #[error("current member signing state is invalid")]
    InvalidState,
    #[error("current member signing state could not be loaded")]
    Repository(#[source] anyhow::Error),
}
