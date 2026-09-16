use thiserror::Error;

use crate::space::lifecycle::RebuildSpaceError;

use uc_core::ports::EngineVersionStateError;

#[derive(Debug, Error)]
pub(crate) enum UpgradeSpaceError {
    #[error("failed to read the previous Engine version")]
    ReadVersion(#[source] EngineVersionStateError),

    #[error("stored Engine version is invalid")]
    InvalidVersion(#[source] semver::Error),

    #[error("failed to read Space setup state during Engine upgrade: {0}")]
    ReadSetupState(String),

    #[error("failed to rebuild space during Engine upgrade")]
    Rebuild(#[source] RebuildSpaceError),

    #[error("Engine upgrade completed but recording its version failed")]
    RecordVersion(#[source] EngineVersionStateError),
}
