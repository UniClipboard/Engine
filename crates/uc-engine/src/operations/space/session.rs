//! 空间会话动作：口令解锁、会话恢复与加密口令变更。
//!
//! 三者共同决定“本机当前是否持有一个可用的加密会话”，因此合并在同一能力簇。

use std::error::Error;

use uc_application::facade::{
    AppFacade, ChangeEncryptionPassphraseError, RecoverSpaceSessionError, SpaceActivityError,
    UnlockSpaceError, UnlockSpaceInput as AppUnlockSpaceInput,
};
use uc_core::crypto::domain::Passphrase;
use uc_observability_contract::{error_source::io_error_kind, uc_error};

use crate::error_codes::*;
use crate::{
    ChangeEncryptionPassphraseInput, EngineError, EngineErrorCategory, OperationResult,
    RecoverSessionInput, UnlockSpaceInput,
};

pub async fn execute_unlock_space(
    facade: &AppFacade,
    input: UnlockSpaceInput,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .unlock_space(AppUnlockSpaceInput {
            passphrase: Passphrase::new(input.passphrase.expose()),
        })
        .await
        .map_err(map_unlock_space_error)?;

    Ok(OperationResult::SpaceUnlocked {
        space_id: result.space_id.as_ref().to_string(),
    })
}

fn map_unlock_space_error(error: UnlockSpaceError) -> EngineError {
    match error {
        UnlockSpaceError::SetupNotCompleted => EngineError::new(
            UNLOCK_SPACE_SETUP_NOT_COMPLETED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        UnlockSpaceError::SpaceNotInitialized => EngineError::new(
            UNLOCK_SPACE_NOT_INITIALIZED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        UnlockSpaceError::WrongPassphrase => EngineError::new(
            UNLOCK_SPACE_UNAUTHORIZED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        UnlockSpaceError::CorruptedKeyMaterial => EngineError::new(
            UNLOCK_SPACE_CORRUPTED_CODE,
            EngineErrorCategory::Internal,
            false,
        ),
        UnlockSpaceError::Internal { .. } => {
            uc_error!(
                error_kind = "unlock_space",
                io_error_kind = io_error_kind(&error),
                "unlock space failed"
            );
            EngineError::new(
                UNLOCK_SPACE_FAILED_CODE,
                EngineErrorCategory::Internal,
                false,
            )
        }
    }
}

pub async fn execute_recover_session(
    facade: &AppFacade,
    input: RecoverSessionInput,
) -> Result<OperationResult, EngineError> {
    if !input.allow_secure_storage_unlock {
        return Ok(OperationResult::SessionRecovered {
            unlocked: false,
            resumed: false,
        });
    }

    let result = facade
        .recover_space_session()
        .await
        .map_err(map_recover_session_error)?;

    Ok(OperationResult::SessionRecovered {
        unlocked: result.unlocked,
        resumed: result.resumed,
    })
}

fn map_recover_session_error(error: RecoverSpaceSessionError) -> EngineError {
    if matches!(
        error,
        RecoverSpaceSessionError::Activity(SpaceActivityError::Receive(_))
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
    error: impl Into<Box<dyn Error + Send + Sync>>,
) -> EngineError {
    let error = error.into();
    uc_error!(
        context = context,
        io_error_kind = io_error_kind(error.as_ref()),
        "engine session recovery failed"
    );
    EngineError::new(code, EngineErrorCategory::Unavailable, true)
}

pub async fn execute_change_encryption_passphrase(
    facade: &AppFacade,
    input: ChangeEncryptionPassphraseInput,
) -> Result<OperationResult, EngineError> {
    facade
        .change_encryption_passphrase(
            &Passphrase::new(input.passphrase.expose()),
            &Passphrase::new(input.passphrase_confirmation.expose()),
        )
        .await
        .map_err(map_passphrase_error)?;
    Ok(OperationResult::EncryptionPassphraseChanged)
}

fn map_passphrase_error(error: ChangeEncryptionPassphraseError) -> EngineError {
    match error {
        ChangeEncryptionPassphraseError::PassphraseMismatch => EngineError::new(
            ENCRYPTION_PASSPHRASE_MISMATCH_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        ChangeEncryptionPassphraseError::Locked => EngineError::new(
            ENCRYPTION_PASSPHRASE_LOCKED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        ChangeEncryptionPassphraseError::MultipleDevices => EngineError::new(
            ENCRYPTION_PASSPHRASE_MULTIPLE_DEVICES_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        ChangeEncryptionPassphraseError::MembershipRecoveryRequired => EngineError::new(
            ENCRYPTION_PASSPHRASE_MEMBERSHIP_RECOVERY_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        ChangeEncryptionPassphraseError::MembershipUnavailable
        | ChangeEncryptionPassphraseError::Invitation { .. }
        | ChangeEncryptionPassphraseError::Unavailable { .. } => {
            uc_error!(
                error_kind = "passphrase_change_unavailable",
                io_error_kind = io_error_kind(&error),
                "encryption passphrase change is unavailable"
            );
            EngineError::new(
                ENCRYPTION_PASSPHRASE_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                true,
            )
        }
        ChangeEncryptionPassphraseError::RecoveryRequired { .. } => {
            uc_error!(
                error_kind = "passphrase_change_recovery",
                io_error_kind = io_error_kind(&error),
                "encryption passphrase change requires recovery"
            );
            EngineError::new(
                ENCRYPTION_PASSPHRASE_CHANGE_RECOVERY_CODE,
                EngineErrorCategory::InvalidState,
                true,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_state_failures_keep_distinct_stable_codes() {
        let setup_incomplete = map_unlock_space_error(UnlockSpaceError::SetupNotCompleted);
        let space_missing = map_unlock_space_error(UnlockSpaceError::SpaceNotInitialized);

        assert_eq!(
            setup_incomplete.category(),
            EngineErrorCategory::InvalidState
        );
        assert_eq!(space_missing.category(), EngineErrorCategory::InvalidState);
        assert_ne!(setup_incomplete.code(), space_missing.code());
    }

    #[test]
    fn corrupted_unlock_material_is_distinct_from_internal_failure() {
        let corrupted = map_unlock_space_error(UnlockSpaceError::CorruptedKeyMaterial);
        let internal = map_unlock_space_error(UnlockSpaceError::Internal {
            source: anyhow::anyhow!("migration failed"),
        });

        assert_ne!(corrupted.code(), internal.code());
    }

    #[test]
    fn receive_recovery_failure_has_a_distinct_stable_code() {
        assert_ne!(
            RECOVER_SESSION_UNAVAILABLE_CODE,
            RECOVER_SESSION_RECEIVE_UNAVAILABLE_CODE
        );
    }

    #[test]
    fn passphrase_eligibility_failures_have_stable_distinct_codes() {
        let mismatch = map_passphrase_error(ChangeEncryptionPassphraseError::PassphraseMismatch);
        let locked = map_passphrase_error(ChangeEncryptionPassphraseError::Locked);
        let multiple = map_passphrase_error(ChangeEncryptionPassphraseError::MultipleDevices);

        assert_eq!(mismatch.code(), ENCRYPTION_PASSPHRASE_MISMATCH_CODE);
        assert_eq!(mismatch.category(), EngineErrorCategory::InvalidInput);
        assert_eq!(locked.code(), ENCRYPTION_PASSPHRASE_LOCKED_CODE);
        assert_eq!(multiple.code(), ENCRYPTION_PASSPHRASE_MULTIPLE_DEVICES_CODE);
        assert_eq!(multiple.category(), EngineErrorCategory::Conflict);
    }
}
