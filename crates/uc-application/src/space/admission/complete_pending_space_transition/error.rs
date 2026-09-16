#[derive(Debug, thiserror::Error)]
pub enum CompletePendingSpaceTransitionError {
    #[error("failed to complete pending Space transition")]
    State {
        #[source]
        source: anyhow::Error,
    },

    #[error("completed Space transition did not produce an active join")]
    JoinNotActive,
}

impl CompletePendingSpaceTransitionError {
    pub(crate) fn state(source: impl Into<anyhow::Error>) -> Self {
        Self::State {
            source: source.into(),
        }
    }
}
