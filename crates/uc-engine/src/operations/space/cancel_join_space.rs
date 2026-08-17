use base64::Engine as _;

use crate::error_codes::{CANCEL_JOIN_SPACE_NOT_FOUND_CODE, JOIN_SPACE_FAILED_CODE};
use crate::operations::device::member::join_space_status;
use crate::{CancelJoinSpaceInput, EngineError, EngineErrorCategory, OperationResult};

pub async fn execute_cancel_join_space(
    convergence: &uc_application::facade::ProfileWorkspaceConvergence,
    input: CancelJoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input.join_id)
        .map_err(|_| not_found())?;
    let join_id: [u8; 16] = bytes.try_into().map_err(|_| not_found())?;
    convergence
        .cancel_join_space(join_id)
        .await
        .map(|status| OperationResult::JoinSpace(join_space_status(status)))
        .map_err(|error| match error {
            uc_application::facade::WorkspaceConvergenceError::JoinNotFound => not_found(),
            _ => EngineError::new(JOIN_SPACE_FAILED_CODE, EngineErrorCategory::Internal, false),
        })
}

fn not_found() -> EngineError {
    EngineError::new(
        CANCEL_JOIN_SPACE_NOT_FOUND_CODE,
        EngineErrorCategory::NotFound,
        false,
    )
}
