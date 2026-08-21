use thiserror::Error;

use crate::error::anyhow_error_constructor;

#[derive(Debug, Error)]
pub(crate) enum RebuildSpaceError {
    #[error("failed to prepare single-device space rebuild")]
    PreparationFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to stage single-device space rebuild")]
    StagingFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to rebuild the single-device space")]
    RebuildFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to commit single-device space rebuild")]
    CommitFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("single-device space rebuild committed but finalization failed")]
    FinalizationFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("local device name is unavailable")]
    DeviceNameUnavailable,

    #[error("clock returned an invalid timestamp")]
    InvalidClock,
}

impl RebuildSpaceError {
    anyhow_error_constructor!(preparation, PreparationFailed);
    anyhow_error_constructor!(staging, StagingFailed);
    anyhow_error_constructor!(rebuild, RebuildFailed);
    anyhow_error_constructor!(commit, CommitFailed);
    anyhow_error_constructor!(finalize, FinalizationFailed);
}
