//! Shared session recovery implementation.
//!
//! The daemon uses this internal seam only while its remaining callers migrate
//! to `Engine`. Do not re-export it from the crate root.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::{AppFacade, SpaceActivityError, SpaceSessionError};

use crate::{EngineError, EngineErrorCategory, OperationResult, RecoverSessionInput};

pub async fn execute_recover_session(
    facade: &AppFacade,
    input: RecoverSessionInput,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .recover_space_session(input.allow_secure_storage_unlock)
        .await
        .map_err(map_recover_session_error)?;

    Ok(OperationResult::SessionRecovered {
        unlocked: result.unlocked,
        resumed: result.resumed,
    })
}

fn map_recover_session_error(error: SpaceSessionError) -> EngineError {
    if matches!(
        error,
        SpaceSessionError::Activity(SpaceActivityError::Receive(_))
    ) {
        return recover_session_error(
            RECOVER_SESSION_RECEIVE_UNAVAILABLE_CODE,
            "restore receive activity",
            error,
        );
    }
    recover_session_error(
        RECOVER_SESSION_UNAVAILABLE_CODE,
        "recover space session",
        error,
    )
}

fn recover_session_error(
    code: u32,
    context: &'static str,
    error: impl std::fmt::Display,
) -> EngineError {
    error!(context, error = %error, "engine session recovery failed");
    EngineError::new(code, EngineErrorCategory::Unavailable, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_recovery_failure_has_a_distinct_stable_code() {
        assert_ne!(
            RECOVER_SESSION_UNAVAILABLE_CODE,
            RECOVER_SESSION_RECEIVE_UNAVAILABLE_CODE
        );
    }
}
