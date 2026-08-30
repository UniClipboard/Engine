//! Shared join-space implementation.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::{
    AppFacade, JoinSpaceError as AppJoinSpaceError, JoinSpaceInput as AppJoinSpaceInput,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::pairing::InvitationCode;

use crate::operations::device::member::join_space_status;

use crate::{EngineError, EngineErrorCategory, JoinSpaceInput, OperationResult};

pub async fn execute_join_space(
    facade: &AppFacade,
    input: JoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    let joined = facade
        .join_space(AppJoinSpaceInput {
            invitation_code: InvitationCode::new(input.invitation_code),
            device_name: input.device_name,
            passphrase: Passphrase::new(input.passphrase.expose()),
            preserve_unreadable_history: input.preserve_unreadable_history,
        })
        .await
        .map_err(map_join_space_error)?;
    Ok(join_status_result(joined.status))
}

pub(crate) fn join_status_result(
    status: uc_application::facade::CurrentJoinStatus,
) -> OperationResult {
    OperationResult::JoinSpace(join_space_status(status))
}

fn map_join_space_error(error: AppJoinSpaceError) -> EngineError {
    match error {
        AppJoinSpaceError::DeviceNameRequired => device_name_required_error(),
        AppJoinSpaceError::InvalidInvitation => error_with(
            JOIN_SPACE_INVITATION_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        AppJoinSpaceError::PreviousJoinCannotBeSuperseded => error_with(
            JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        AppJoinSpaceError::Locked => error_with(
            JOIN_SPACE_NOT_UNLOCKED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        AppJoinSpaceError::StateChanged => {
            error_with(JOIN_SPACE_STORAGE_CODE, EngineErrorCategory::Conflict, true)
        }
        AppJoinSpaceError::RecoveryRequired => error_with(
            JOIN_SPACE_PENDING_MIGRATION_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        AppJoinSpaceError::Unavailable => unavailable_error(JOIN_SPACE_STORAGE_CODE),
        AppJoinSpaceError::Settings(_) | AppJoinSpaceError::InvalidStartMaterial => {
            join_internal_error("join space", error)
        }
    }
}

fn device_name_required_error() -> EngineError {
    error_with(
        JOIN_SPACE_DEVICE_NAME_REQUIRED_CODE,
        EngineErrorCategory::InvalidInput,
        false,
    )
}

fn unavailable_error(code: u32) -> EngineError {
    error_with(code, EngineErrorCategory::Unavailable, true)
}

fn error_with(code: u32, category: EngineErrorCategory, retryable: bool) -> EngineError {
    EngineError::new(code, category, retryable)
}

fn join_internal_error(context: &'static str, error: impl std::fmt::Display) -> EngineError {
    error!(context, error = %error, "join-space operation failed");
    error_with(JOIN_SPACE_FAILED_CODE, EngineErrorCategory::Internal, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_start_failures_keep_stable_product_categories() {
        let locked = map_join_space_error(AppJoinSpaceError::Locked);
        let changed = map_join_space_error(AppJoinSpaceError::StateChanged);
        let recovery = map_join_space_error(AppJoinSpaceError::RecoveryRequired);
        let unavailable = map_join_space_error(AppJoinSpaceError::Unavailable);

        assert_eq!(locked.code(), JOIN_SPACE_NOT_UNLOCKED_CODE);
        assert_eq!(locked.category(), EngineErrorCategory::Unauthorized);
        assert_eq!(changed.category(), EngineErrorCategory::Conflict);
        assert!(changed.is_retryable());
        assert_eq!(recovery.category(), EngineErrorCategory::InvalidState);
        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert!(unavailable.is_retryable());
    }

    #[test]
    fn previous_join_cannot_be_superseded_is_a_stable_conflict() {
        let error = map_join_space_error(AppJoinSpaceError::PreviousJoinCannotBeSuperseded);

        assert_eq!(
            error.code(),
            JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE
        );
        assert_eq!(error.category(), EngineErrorCategory::Conflict);
        assert!(!error.is_retryable());
    }
}
