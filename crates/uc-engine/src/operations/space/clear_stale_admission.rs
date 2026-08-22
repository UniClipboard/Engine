//! Lightweight \"clear stale pairing state\" operation.
//!
//! A crash or interruption during a pairing (join / sponsor) leaves a durable
//! \"admission in progress\" record in the admission store. That record blocks
//! every later pairing attempt until it is cleared. This operation clears only
//! the pending attempts — it does NOT touch the space, its membership history,
//! completed terminals or the device trust revision, so an otherwise intact
//! space keeps working and users can simply re-pair.

use crate::error_codes::{
    CLEAR_STALE_ADMISSION_FAILED_CODE, CLEAR_STALE_ADMISSION_UNAVAILABLE_CODE,
};
use crate::{EngineError, EngineErrorCategory, OperationResult};

use tracing::error;
use uc_application::facade::ProfileWorkspaceConvergence;

pub async fn execute_clear_stale_admission(
    profile_convergence: &ProfileWorkspaceConvergence,
) -> Result<OperationResult, EngineError> {
    profile_convergence
        .clear_stale_admission()
        .await
        .map(|()| OperationResult::StaleAdmissionCleared)
        .map_err(|e| {
            error!(error = %e, "clear stale admission failed");
            match e {
                uc_application::facade::WorkspaceConvergenceError::Locked
                | uc_application::facade::WorkspaceConvergenceError::Unavailable => {
                    EngineError::new(
                        CLEAR_STALE_ADMISSION_UNAVAILABLE_CODE,
                        EngineErrorCategory::Unavailable,
                        true,
                    )
                }
                _ => EngineError::new(
                    CLEAR_STALE_ADMISSION_FAILED_CODE,
                    EngineErrorCategory::Internal,
                    false,
                ),
            }
        })
}