#[derive(Debug, thiserror::Error)]
#[error("failed to recover Space join completion: {0}")]
pub struct RecoverSpaceJoinCompletionError(pub(crate) String);
