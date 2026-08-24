#[derive(Debug, thiserror::Error)]
pub enum CompletePendingSpaceTransitionError {
    #[error("failed to complete pending Space transition: {0}")]
    State(String),

    #[error("completed Space transition did not produce an active join")]
    JoinNotActive,
}
