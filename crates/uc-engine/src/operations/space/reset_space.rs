//! Shared space reset implementation.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::{
    AppFacade, ProfileWorkspaceConvergence, ResetSpaceError, WorkspaceConvergenceError,
};

use crate::{EngineError, EngineErrorCategory, OperationResult};

pub async fn execute_reset_space(
    convergence: &ProfileWorkspaceConvergence,
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    convergence
        .prepare_reset_space()
        .await
        .map_err(map_reset_preparation_error)?;
    facade.reset_space().await.map_err(|error| match error {
        ResetSpaceError::StorageFailed(_) | ResetSpaceError::Internal(_) => {
            error!(error = %error, "reset space failed");
            EngineError::new(
                RESET_SPACE_FAILED_CODE,
                EngineErrorCategory::Internal,
                false,
            )
        }
    })?;
    Ok(OperationResult::SpaceReset)
}

fn map_reset_preparation_error(error: WorkspaceConvergenceError) -> EngineError {
    if matches!(error, WorkspaceConvergenceError::Unavailable) {
        return EngineError::new(
            RESET_SPACE_UNAVAILABLE_CODE,
            EngineErrorCategory::Conflict,
            false,
        );
    }
    error!(error = %error, "reset space admission check failed");
    EngineError::new(
        RESET_SPACE_FAILED_CODE,
        EngineErrorCategory::Internal,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_admission_maps_to_the_existing_reset_conflict() {
        let error = map_reset_preparation_error(WorkspaceConvergenceError::Unavailable);

        assert_eq!(error.code(), RESET_SPACE_UNAVAILABLE_CODE);
        assert_eq!(error.category(), EngineErrorCategory::Conflict);
    }
}
