#[derive(Debug, thiserror::Error)]
pub enum QuerySetupStateError {
    #[error("failed to read setup state")]
    StorageFailed(#[source] anyhow::Error),

    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}
