#[derive(Debug, thiserror::Error)]
#[error("failed to query pending Space transition: {0}")]
pub struct QueryPendingSpaceTransitionError(pub(crate) String);
