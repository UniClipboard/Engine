use tracing::error;
use uc_application::facade::{AppFacade, ChangeEncryptionPassphraseError};
use uc_core::crypto::domain::Passphrase;

use crate::error_codes::*;
use crate::{
    ConfirmEncryptionPassphraseChangeInput, EngineError, EngineErrorCategory, OperationResult,
    SecretString,
};

pub async fn execute_generate_encryption_passphrase(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let passphrase = facade
        .generate_encryption_passphrase()
        .await
        .map_err(map_error)?;
    Ok(OperationResult::EncryptionPassphraseGenerated {
        passphrase: SecretString::new(passphrase.expose()),
    })
}

pub async fn execute_confirm_encryption_passphrase_change(
    facade: &AppFacade,
    input: ConfirmEncryptionPassphraseChangeInput,
) -> Result<OperationResult, EngineError> {
    facade
        .confirm_encryption_passphrase_change(&Passphrase::new(input.passphrase.expose()))
        .await
        .map_err(map_error)?;
    Ok(OperationResult::EncryptionPassphraseChanged)
}

fn map_error(error: ChangeEncryptionPassphraseError) -> EngineError {
    match error {
        ChangeEncryptionPassphraseError::NotGenerated => EngineError::new(
            ENCRYPTION_PASSPHRASE_NOT_GENERATED_CODE,
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
            error!(error = %error, "encryption passphrase change is unavailable");
            EngineError::new(
                ENCRYPTION_PASSPHRASE_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                true,
            )
        }
        ChangeEncryptionPassphraseError::RecoveryRequired { .. } => {
            error!(error = %error, "encryption passphrase change requires recovery");
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
    fn eligibility_failures_have_stable_distinct_codes() {
        let not_generated = map_error(ChangeEncryptionPassphraseError::NotGenerated);
        let locked = map_error(ChangeEncryptionPassphraseError::Locked);
        let multiple = map_error(ChangeEncryptionPassphraseError::MultipleDevices);

        assert_eq!(
            not_generated.code(),
            ENCRYPTION_PASSPHRASE_NOT_GENERATED_CODE
        );
        assert_eq!(not_generated.category(), EngineErrorCategory::InvalidInput);
        assert_eq!(locked.code(), ENCRYPTION_PASSPHRASE_LOCKED_CODE);
        assert_eq!(multiple.code(), ENCRYPTION_PASSPHRASE_MULTIPLE_DEVICES_CODE);
        assert_eq!(multiple.category(), EngineErrorCategory::Conflict);
    }
}
