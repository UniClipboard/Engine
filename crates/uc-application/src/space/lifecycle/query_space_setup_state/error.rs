#[derive(Debug, thiserror::Error)]
pub enum QuerySetupStateError {
    #[error("failed to read setup state: {0}")]
    StorageFailed(String),

    #[error("internal error: {0}")]
    Internal(String),
}
