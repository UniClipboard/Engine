//! Shared join-space implementation.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::space_setup::{SwitchSpaceError, SwitchSpaceResult};
use uc_application::facade::{
    AppFacade, JoinSpaceError as AppJoinSpaceError, JoinSpaceInput as AppJoinSpaceInput,
    JoinSpaceResult as AppJoinSpaceResult, RedeemPairingInvitationError,
    RedeemPairingInvitationResult,
};

use crate::{EngineError, EngineErrorCategory, JoinSpaceInput, OperationResult};

pub async fn execute_join_space(
    facade: &AppFacade,
    input: JoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .join_space(AppJoinSpaceInput {
            invitation_code: input.invitation_code,
            device_name: input.device_name,
            passphrase: input.passphrase.expose().to_owned(),
            preserve_unreadable_history: input.preserve_unreadable_history,
        })
        .await
        .map_err(map_join_space_error)?;
    Ok(match result {
        AppJoinSpaceResult::Fresh(result) => fresh_join_result(result),
        AppJoinSpaceResult::Switched(result) => switch_join_result(result),
    })
}

fn map_join_space_error(error: AppJoinSpaceError) -> EngineError {
    match error {
        AppJoinSpaceError::DeviceNameRequired => device_name_required_error(),
        AppJoinSpaceError::Fresh(error) => map_fresh_join_error(error),
        AppJoinSpaceError::Switch(error) => map_switch_space_error(error),
        AppJoinSpaceError::Settings(_)
        | AppJoinSpaceError::Setup(_)
        | AppJoinSpaceError::Activity(_) => join_internal_error("join space", error),
    }
}

fn fresh_join_result(result: RedeemPairingInvitationResult) -> OperationResult {
    OperationResult::SpaceJoined {
        sponsor_device_id: result.sponsor_device_id.to_string(),
        sponsor_identity_fingerprint: result.sponsor_identity_fingerprint.as_display().to_string(),
        space_id: result.space_id.as_ref().to_string(),
        self_device_id: result.self_device_id.to_string(),
        self_identity_fingerprint: result.self_identity_fingerprint.as_display().to_string(),
        migrated_records: None,
        preserved_unreadable_records: None,
    }
}

fn switch_join_result(result: SwitchSpaceResult) -> OperationResult {
    OperationResult::SpaceJoined {
        sponsor_device_id: result.sponsor_device_id.to_string(),
        sponsor_identity_fingerprint: result.sponsor_identity_fingerprint.as_display().to_string(),
        space_id: result.space_id.as_ref().to_string(),
        self_device_id: result.self_device_id.to_string(),
        self_identity_fingerprint: result.self_identity_fingerprint.as_display().to_string(),
        migrated_records: Some(result.migrated_records),
        preserved_unreadable_records: Some(result.preserved_unreadable_records),
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
        RedeemPairingInvitationError::SponsorRejectedInvitation => error_with(
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

fn map_switch_space_error(error: SwitchSpaceError) -> EngineError {
    match error {
        SwitchSpaceError::NotSetup => error_with(
            JOIN_SPACE_NOT_SETUP_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        SwitchSpaceError::PendingMigration(_) => error_with(
            JOIN_SPACE_PENDING_MIGRATION_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        SwitchSpaceError::NotUnlocked => error_with(
            JOIN_SPACE_NOT_UNLOCKED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        SwitchSpaceError::InvitationNotFound => error_with(
            JOIN_SPACE_INVITATION_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        SwitchSpaceError::InvitationExpired => error_with(
            JOIN_SPACE_INVITATION_EXPIRED_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        SwitchSpaceError::SponsorUnreachable => {
            unavailable_error(JOIN_SPACE_SPONSOR_UNREACHABLE_CODE)
        }
        SwitchSpaceError::SponsorDeclined => error_with(
            JOIN_SPACE_SPONSOR_DECLINED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        SwitchSpaceError::SponsorRejectedInvitation => error_with(
            JOIN_SPACE_SPONSOR_REJECTED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        SwitchSpaceError::Timeout => deadline_error(JOIN_SPACE_TIMEOUT_CODE),
        SwitchSpaceError::ConnectionLost => unavailable_error(JOIN_SPACE_CONNECTION_LOST_CODE),
        SwitchSpaceError::PassphraseMismatch => error_with(
            JOIN_SPACE_PASSPHRASE_MISMATCH_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        SwitchSpaceError::CorruptedKeyMaterial => error_with(
            JOIN_SPACE_CORRUPTED_KEY_CODE,
            EngineErrorCategory::Internal,
            false,
        ),
        SwitchSpaceError::DeviceNameRequired => device_name_required_error(),
        SwitchSpaceError::ServiceUnavailable => {
            unavailable_error(JOIN_SPACE_SERVICE_UNAVAILABLE_CODE)
        }
        SwitchSpaceError::SponsorUpgradeRequired => error_with(
            JOIN_SPACE_SPONSOR_UPGRADE_REQUIRED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        SwitchSpaceError::UnreadableHistoryRequiresConfirmation => error_with(
            JOIN_SPACE_UNREADABLE_HISTORY_REQUIRES_CONFIRMATION_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        SwitchSpaceError::InvalidCiphertext => error_with(
            JOIN_SPACE_INVALID_CIPHERTEXT_CODE,
            EngineErrorCategory::Internal,
            false,
        ),
        SwitchSpaceError::Storage(_) => {
            error!(error = %error, "switch-space storage failed");
            error_with(
                JOIN_SPACE_STORAGE_CODE,
                EngineErrorCategory::Internal,
                false,
            )
        }
        SwitchSpaceError::Internal(_) => join_internal_error("switch space", error),
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

        assert_ne!(not_found.code(), expired.code());
        assert_ne!(rejected.code(), declined.code());
    }

    #[test]
    fn switch_precondition_failures_keep_user_visible_reasons_distinct() {
        let not_setup = map_switch_space_error(SwitchSpaceError::NotSetup);
        let pending = map_switch_space_error(SwitchSpaceError::PendingMigration(
            uc_core::setup::MigrationPhase::Prepared {
                run_id: uc_core::setup::MigrationRunId::new("run-1"),
                preserved_unreadable_records: 0,
            },
        ));
        let locked = map_switch_space_error(SwitchSpaceError::NotUnlocked);

        assert_ne!(not_setup.code(), pending.code());
        assert_ne!(pending.code(), locked.code());
    }

    #[test]
    fn unreadable_history_confirmation_has_a_distinct_public_error() {
        let error = map_switch_space_error(SwitchSpaceError::UnreadableHistoryRequiresConfirmation);

        assert_eq!(
            error.code(),
            JOIN_SPACE_UNREADABLE_HISTORY_REQUIRES_CONFIRMATION_CODE
        );
        assert_eq!(error.category(), EngineErrorCategory::Conflict);
        assert!(!error.is_retryable());
    }
}
