//! Shared factory-reset operation.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::{ProfileFactoryReset, ProfileFactoryResetError};

use crate::{EngineError, EngineErrorCategory, OperationResult};

pub async fn execute_factory_reset_space(
    reset: &ProfileFactoryReset,
) -> Result<OperationResult, EngineError> {
    reset
        .factory_reset()
        .await
        .map_err(map_profile_factory_reset_error)?;
    Ok(OperationResult::SpaceFactoryReset)
}

pub(crate) fn map_profile_factory_reset_error(error: ProfileFactoryResetError) -> EngineError {
    let code = match error {
        ProfileFactoryResetError::StopRuntime => FACTORY_RESET_UNAVAILABLE_CODE,
        ProfileFactoryResetError::WipeKeys => FACTORY_RESET_KEY_MATERIAL_FAILED_CODE,
        ProfileFactoryResetError::ClearState => FACTORY_RESET_STORAGE_FAILED_CODE,
        ProfileFactoryResetError::Lifecycle(_) => FACTORY_RESET_FAILED_CODE,
    };
    error!(code, error = %error, "factory reset space failed");
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_reset_failures_keep_distinct_stable_codes() {
        let key_material = map_profile_factory_reset_error(ProfileFactoryResetError::WipeKeys);
        let storage = map_profile_factory_reset_error(ProfileFactoryResetError::ClearState);
        let internal = map_profile_factory_reset_error(ProfileFactoryResetError::Lifecycle(
            uc_core::ports::ProfileLifecycleError::Corrupt,
        ));

        assert_ne!(key_material.code(), storage.code());
        assert_ne!(storage.code(), internal.code());
        assert_ne!(key_material.code(), internal.code());
    }
}
