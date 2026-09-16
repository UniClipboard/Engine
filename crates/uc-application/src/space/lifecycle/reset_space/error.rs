use thiserror::Error;

use crate::space::lifecycle::RebuildSpaceError;

#[derive(Debug, Error)]
/// Failure modes of resetting the current profile to a single-device Space.
pub enum ResetSpaceError {
    #[error("failed to prepare device management reset: {0}")]
    PreparationFailed(String),

    #[error("failed to stage device management reset: {0}")]
    StagingFailed(String),

    #[error("failed to rebuild the single-device space: {0}")]
    RebuildFailed(String),

    #[error("failed to commit device management reset: {0}")]
    CommitFailed(String),

    #[error("device management reset committed but finalization failed: {0}")]
    FinalizationFailed(String),

    /// Uncategorised infra / adapter failure.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<RebuildSpaceError> for ResetSpaceError {
    fn from(error: RebuildSpaceError) -> Self {
        match error {
            RebuildSpaceError::PreparationFailed { source } => {
                Self::PreparationFailed(source.to_string())
            }
            RebuildSpaceError::StagingFailed { source } => Self::StagingFailed(source.to_string()),
            RebuildSpaceError::RebuildFailed { source } => Self::RebuildFailed(source.to_string()),
            RebuildSpaceError::CommitFailed { source } => Self::CommitFailed(source.to_string()),
            RebuildSpaceError::FinalizationFailed { source } => {
                Self::FinalizationFailed(source.to_string())
            }
            RebuildSpaceError::DeviceNameUnavailable => {
                Self::PreparationFailed("local device name is unavailable".to_owned())
            }
            RebuildSpaceError::InvalidClock => {
                Self::RebuildFailed("clock returned an invalid timestamp".to_owned())
            }
        }
    }
}
