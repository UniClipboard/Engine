use crate::space::lifecycle::SpaceActivityError;

#[derive(Debug, thiserror::Error)]
pub enum RecoverSpaceSessionError {
    #[error("failed to load current Space identity")]
    CurrentSpace(#[source] anyhow::Error),
    #[error("cached master key is not available from secure storage")]
    KeyringMiss,
    #[error("space key material corrupted")]
    CorruptedKeyMaterial,
    #[error(transparent)]
    Activity(#[from] SpaceActivityError),
    #[error("space session recovery failed")]
    Internal(#[source] anyhow::Error),
}
