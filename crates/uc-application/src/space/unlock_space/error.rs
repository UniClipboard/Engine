use thiserror::Error;

#[derive(Debug, Error)]
pub enum UnlockSpaceError {
    #[error("setup has not been completed")]
    SetupNotCompleted,

    #[error("space is not initialised")]
    SpaceNotInitialized,

    #[error("wrong passphrase")]
    WrongPassphrase,

    #[error("space key material corrupted")]
    CorruptedKeyMaterial,

    #[error("internal error: {0}")]
    Internal(String),
}
