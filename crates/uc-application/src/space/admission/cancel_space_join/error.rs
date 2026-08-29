#[derive(Debug, thiserror::Error)]
pub enum CancelSpaceJoinError {
    #[error("Space join was not found")]
    NotFound,

    #[error("failed to cancel Space join")]
    State {
        #[source]
        source: anyhow::Error,
    },
}

impl CancelSpaceJoinError {
    pub(crate) fn state(source: impl Into<anyhow::Error>) -> Self {
        Self::State {
            source: source.into(),
        }
    }
}
