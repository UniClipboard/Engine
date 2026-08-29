#[derive(Debug, thiserror::Error)]
#[error("failed to query pending Space transition")]
pub struct QueryPendingSpaceTransitionError {
    #[source]
    source: anyhow::Error,
}

impl QueryPendingSpaceTransitionError {
    pub(crate) fn state(source: impl Into<anyhow::Error>) -> Self {
        Self {
            source: source.into(),
        }
    }
}
