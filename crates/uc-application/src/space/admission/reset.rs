use std::sync::Arc;

use crate::deps::AdmissionAttemptRepositoryError;
use async_trait::async_trait;
use thiserror::Error;

use crate::deps::AdmissionAttemptRepositoryPort;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceAdmissionResetError {
    #[error("space admission state is unavailable")]
    Unavailable,

    #[error("space admission state is inconsistent")]
    Inconsistent,
}

#[async_trait]
pub trait SpaceAdmissionResetPort: Send + Sync {
    /// Clear admission state that belongs to the previously active Space.
    ///
    /// Repeated calls must succeed.
    async fn clear_prior_space_state(&self) -> Result<(), SpaceAdmissionResetError>;
}

pub(crate) struct PriorSpaceAdmissionStateReset {
    repository: Arc<dyn AdmissionAttemptRepositoryPort>,
}

impl PriorSpaceAdmissionStateReset {
    pub(crate) fn new(repository: Arc<dyn AdmissionAttemptRepositoryPort>) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl SpaceAdmissionResetPort for PriorSpaceAdmissionStateReset {
    async fn clear_prior_space_state(&self) -> Result<(), SpaceAdmissionResetError> {
        self.repository
            .reset_admission_profile()
            .await
            .map(|_| ())
            .map_err(map_repository_error)
    }
}

fn map_repository_error(error: AdmissionAttemptRepositoryError) -> SpaceAdmissionResetError {
    match error {
        AdmissionAttemptRepositoryError::Corrupt
        | AdmissionAttemptRepositoryError::AlreadyExists
        | AdmissionAttemptRepositoryError::NotFound
        | AdmissionAttemptRepositoryError::VersionConflict
        | AdmissionAttemptRepositoryError::PreviousJoinCannotBeSuperseded => {
            SpaceAdmissionResetError::Inconsistent
        }
        AdmissionAttemptRepositoryError::Locked
        | AdmissionAttemptRepositoryError::Repository(_) => SpaceAdmissionResetError::Unavailable,
    }
}
