use std::sync::Arc;

use super::QueryPendingSpaceTransitionError;
use crate::deps::AdmissionAttemptRepositoryPort;
use crate::space::admission::durable;

pub(crate) struct QueryPendingSpaceTransitionUseCase {
    admission_attempts: Arc<dyn AdmissionAttemptRepositoryPort>,
}

impl QueryPendingSpaceTransitionUseCase {
    pub(crate) fn new(admission_attempts: Arc<dyn AdmissionAttemptRepositoryPort>) -> Self {
        Self { admission_attempts }
    }

    pub(crate) async fn execute(&self) -> Result<bool, QueryPendingSpaceTransitionError> {
        let attempts = self
            .admission_attempts
            .scan_recoverable()
            .await
            .map_err(durable::map_repository_error)
            .map_err(|error| QueryPendingSpaceTransitionError(error.to_string()))?;
        Ok(attempts.into_iter().any(|attempt| {
            attempt.is_joiner()
                && attempt.completion.is_some()
                && attempt.space_transition.is_some()
                && attempt.space_transition_result.is_none()
        }))
    }
}
