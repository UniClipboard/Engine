use thiserror::Error;

use crate::space::lifecycle::RebuildSpaceError;

#[derive(Debug, Error)]
/// Failure modes of resetting the current profile to a single-device Space.
pub enum ResetSpaceError {
    #[error("failed to prepare device management reset")]
    PreparationFailed(#[source] anyhow::Error),

    #[error("failed to stage device management reset")]
    StagingFailed(#[source] anyhow::Error),

    #[error("failed to rebuild the single-device space")]
    RebuildFailed(#[source] anyhow::Error),

    #[error("failed to commit device management reset")]
    CommitFailed(#[source] anyhow::Error),

    #[error("device management reset committed but finalization failed")]
    FinalizationFailed(#[source] anyhow::Error),

    /// Uncategorised infra / adapter failure.
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl From<RebuildSpaceError> for ResetSpaceError {
    fn from(error: RebuildSpaceError) -> Self {
        match error {
            RebuildSpaceError::PreparationFailed { source } => Self::PreparationFailed(source),
            RebuildSpaceError::StagingFailed { source } => Self::StagingFailed(source),
            RebuildSpaceError::RebuildFailed { source } => Self::RebuildFailed(source),
            RebuildSpaceError::CommitFailed { source } => Self::CommitFailed(source),
            RebuildSpaceError::FinalizationFailed { source } => Self::FinalizationFailed(source),
            error @ RebuildSpaceError::DeviceNameUnavailable => {
                Self::PreparationFailed(error.into())
            }
            error @ RebuildSpaceError::InvalidClock => Self::RebuildFailed(error.into()),
        }
    }
}
