//! Shared join-space implementation.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::{
    AppFacade, JoinSpaceError as AppJoinSpaceError, JoinSpaceInput as AppJoinSpaceInput,
    ProfileSpaceAdmission, RedeemPairingInvitationError,
};

use crate::operations::device::member::join_space_status;

use crate::{EngineError, EngineErrorCategory, JoinSpaceInput, OperationResult};

pub async fn execute_join_space(
    facade: &AppFacade,
    convergence: &ProfileSpaceAdmission,
    input: JoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    facade
        .join_space(AppJoinSpaceInput {
            invitation_code: input.invitation_code,
            device_name: input.device_name,
            passphrase: input.passphrase.expose().to_owned(),
            preserve_unreadable_history: input.preserve_unreadable_history,
        })
        .await
        .map_err(map_join_space_error)?;
    current_join_result(convergence).await
}

pub(crate) async fn current_join_result(
    convergence: &ProfileSpaceAdmission,
) -> Result<OperationResult, EngineError> {
    convergence
        .current_join()
        .await
        .map_err(|error| join_internal_error("query joined space", error))?
        .map(join_space_status)
        .map(OperationResult::JoinSpace)
        .ok_or_else(|| join_internal_error("query joined space", "join result was not persisted"))
}

fn map_join_space_error(error: AppJoinSpaceError) -> EngineError {
    match error {
        AppJoinSpaceError::DeviceNameRequired => device_name_required_error(),
        AppJoinSpaceError::Admission(error) => map_fresh_join_error(error),
        AppJoinSpaceError::Settings(_) => join_internal_error("join space", error),
    }
}

fn map_fresh_join_error(error: RedeemPairingInvitationError) -> EngineError {
    match error {
        RedeemPairingInvitationError::InvitationNotFound => error_with(
            JOIN_SPACE_INVITATION_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        RedeemPairingInvitationError::InvitationExpired => error_with(
            JOIN_SPACE_INVITATION_EXPIRED_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        RedeemPairingInvitationError::SponsorUnreachable => {
            unavailable_error(JOIN_SPACE_SPONSOR_UNREACHABLE_CODE)
        }
        RedeemPairingInvitationError::ServiceUnavailable => {
            unavailable_error(JOIN_SPACE_SERVICE_UNAVAILABLE_CODE)
        }
        RedeemPairingInvitationError::SponsorUpgradeRequired => error_with(
            JOIN_SPACE_SPONSOR_UPGRADE_REQUIRED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        RedeemPairingInvitationError::PassphraseMismatch => error_with(
            JOIN_SPACE_PASSPHRASE_MISMATCH_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        RedeemPairingInvitationError::CorruptedKeyMaterial => error_with(
            JOIN_SPACE_CORRUPTED_KEY_CODE,
            EngineErrorCategory::Internal,
            false,
        ),
        RedeemPairingInvitationError::DeviceNameRequired => device_name_required_error(),
        RedeemPairingInvitationError::UnreadableHistoryRequiresConfirmation => error_with(
            JOIN_SPACE_UNREADABLE_HISTORY_REQUIRES_CONFIRMATION_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        RedeemPairingInvitationError::PreviousJoinCannotBeSuperseded => error_with(
            JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        RedeemPairingInvitationError::SponsorRejectedInvitation => error_with(
            JOIN_SPACE_SPONSOR_REJECTED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        RedeemPairingInvitationError::SponsorAdmissionUnavailable => error_with(
            JOIN_SPACE_SPONSOR_ADMISSION_UNAVAILABLE_CODE,
            EngineErrorCategory::Conflict,
            true,
        ),
        RedeemPairingInvitationError::SponsorAdmissionConflict => error_with(
            JOIN_SPACE_SPONSOR_REJECTED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        RedeemPairingInvitationError::SponsorDeclined => error_with(
            JOIN_SPACE_SPONSOR_DECLINED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        RedeemPairingInvitationError::SponsorTimedOut => {
            deadline_error(JOIN_SPACE_SPONSOR_TIMEOUT_CODE)
        }
        RedeemPairingInvitationError::Timeout => deadline_error(JOIN_SPACE_TIMEOUT_CODE),
        RedeemPairingInvitationError::ConnectionLost => {
            unavailable_error(JOIN_SPACE_CONNECTION_LOST_CODE)
        }
        RedeemPairingInvitationError::SponsorInternal(_)
        | RedeemPairingInvitationError::Internal(_) => join_internal_error("join space", error),
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

fn deadline_error(code: u32) -> EngineError {
    error_with(code, EngineErrorCategory::DeadlineExceeded, true)
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
    fn fresh_join_failures_keep_user_visible_reasons_distinct() {
        let not_found = map_fresh_join_error(RedeemPairingInvitationError::InvitationNotFound);
        let expired = map_fresh_join_error(RedeemPairingInvitationError::InvitationExpired);
        let rejected =
            map_fresh_join_error(RedeemPairingInvitationError::SponsorRejectedInvitation);
        let declined = map_fresh_join_error(RedeemPairingInvitationError::SponsorDeclined);
        let unreadable = map_fresh_join_error(
            RedeemPairingInvitationError::UnreadableHistoryRequiresConfirmation,
        );

        assert_ne!(not_found.code(), expired.code());
        assert_ne!(rejected.code(), declined.code());
        assert_eq!(
            unreadable.code(),
            JOIN_SPACE_UNREADABLE_HISTORY_REQUIRES_CONFIRMATION_CODE
        );
        assert_eq!(unreadable.category(), EngineErrorCategory::Conflict);
        assert!(!unreadable.is_retryable());
    }

    #[test]
    fn previous_join_cannot_be_superseded_is_a_stable_conflict() {
        let error =
            map_fresh_join_error(RedeemPairingInvitationError::PreviousJoinCannotBeSuperseded);

        assert_eq!(
            error.code(),
            JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE
        );
        assert_eq!(error.category(), EngineErrorCategory::Conflict);
        assert!(!error.is_retryable());
    }
}
