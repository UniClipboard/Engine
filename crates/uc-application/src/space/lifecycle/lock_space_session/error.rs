use crate::space::lifecycle::SpaceActivityError;

#[derive(Debug, thiserror::Error)]
pub enum LockSpaceSessionError {
    #[error("failed to load current Space identity")]
    CurrentSpace(#[source] anyhow::Error),
    #[error("current Space is not initialized")]
    NotInitialized,
    #[error(transparent)]
    Activity(#[from] SpaceActivityError),
    #[error("space lock failed")]
    LockFailed,
    #[error("space lock failed and activity recovery was incomplete")]
    RecoveryFailed(#[source] anyhow::Error),
}
