#[derive(Debug, thiserror::Error)]
pub enum CancelSpaceJoinError {
    #[error("Space join was not found")]
    NotFound,

    #[error("failed to cancel Space join: {0}")]
    State(String),
}
