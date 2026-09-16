use crate::error::anyhow_error_constructor;

#[derive(Debug, thiserror::Error)]
pub enum InitializeSpaceError {
    #[error("passphrase and confirmation do not match")]
    PassphraseMismatch,

    #[error("device name is required but not provided")]
    DeviceNameRequired,

    #[error("space is already initialised")]
    AlreadyInitialized,

    #[error("setup has already been completed on this device")]
    AlreadySetup,

    #[error("space initialization storage failed")]
    StorageFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("space initialization failed")]
    Internal {
        #[source]
        source: anyhow::Error,
    },
}

impl InitializeSpaceError {
    anyhow_error_constructor!(storage, StorageFailed);
    anyhow_error_constructor!(internal, Internal);
}
