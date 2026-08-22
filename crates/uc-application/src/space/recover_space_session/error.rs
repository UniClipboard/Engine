use crate::space::session::SpaceActivityError;

#[derive(Debug, thiserror::Error)]
pub enum RecoverSpaceSessionError {
    #[error("failed to load current Space identity: {0}")]
    CurrentSpace(String),
    #[error("cached master key is not available from secure storage")]
    KeyringMiss,
    #[error("space key material corrupted")]
    CorruptedKeyMaterial,
    #[error(transparent)]
    Activity(#[from] SpaceActivityError),
    #[error("space session recovery failed: {0}")]
    Internal(String),
}
