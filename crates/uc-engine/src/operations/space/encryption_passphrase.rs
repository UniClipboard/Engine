use tracing::error;
use uc_application::facade::{AppFacade, ChangeEncryptionPassphraseError};
use uc_core::crypto::domain::Passphrase;

use crate::error_codes::*;
use crate::{ChangeEncryptionPassphraseInput, EngineError, EngineErrorCategory, OperationResult};

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
        .map_err(map_error)?;
    Ok(OperationResult::EncryptionPassphraseChanged)
}

fn map_error(error: ChangeEncryptionPassphraseError) -> EngineError {
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
        let mismatch = map_error(ChangeEncryptionPassphraseError::PassphraseMismatch);
        let locked = map_error(ChangeEncryptionPassphraseError::Locked);
        let multiple = map_error(ChangeEncryptionPassphraseError::MultipleDevices);

        assert_eq!(mismatch.code(), ENCRYPTION_PASSPHRASE_MISMATCH_CODE);
        assert_eq!(mismatch.category(), EngineErrorCategory::InvalidInput);
        assert_eq!(locked.code(), ENCRYPTION_PASSPHRASE_LOCKED_CODE);
        assert_eq!(multiple.code(), ENCRYPTION_PASSPHRASE_MULTIPLE_DEVICES_CODE);
        assert_eq!(multiple.category(), EngineErrorCategory::Conflict);
    }
}
