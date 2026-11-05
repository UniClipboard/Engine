use super::*;

impl From<SecureStorageError> for HostCapabilityError {
    fn from(error: SecureStorageError) -> Self {
        let category = match error {
            SecureStorageError::Unavailable(_) => HostCapabilityErrorCategory::Unavailable,
            SecureStorageError::PermissionDenied(_) => {
                HostCapabilityErrorCategory::PermissionDenied
            }
            _ => HostCapabilityErrorCategory::Io,
        };
        Self::new(category, "profile recovery storage failed")
    }
}

impl From<ProfileKeyRecoveryError> for EngineError {
    fn from(error: ProfileKeyRecoveryError) -> Self {
        match error {
            ProfileKeyRecoveryError::WrongPassphrase => Self::new(
                UNLOCK_SPACE_UNAUTHORIZED_CODE,
                EngineErrorCategory::Unauthorized,
                false,
            ),
            ProfileKeyRecoveryError::Corrupt => Self::new(
                UNLOCK_SPACE_CORRUPTED_CODE,
                EngineErrorCategory::Internal,
                false,
            ),
            ProfileKeyRecoveryError::Unsupported => Self::new(
                PROFILE_RECOVERY_UNSUPPORTED_CODE,
                EngineErrorCategory::Unavailable,
                false,
            ),
            ProfileKeyRecoveryError::Storage(_) => Self::new(
                PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE,
                EngineErrorCategory::Unavailable,
                true,
            ),
        }
    }
}

pub(crate) fn restart_required_error(error: EngineError) -> EngineError {
    EngineError::new(error.code(), error.category(), false)
}

pub(crate) fn public_losses(losses: ProfileRecoveryLosses) -> Vec<ProfileRecoveryLoss> {
    let mut result = Vec::new();
    if losses.local_history {
        result.push(ProfileRecoveryLoss::LocalHistory);
    }
    if losses.local_control_state {
        result.push(ProfileRecoveryLoss::LocalControlState);
    }
    if losses.device_identity {
        result.push(ProfileRecoveryLoss::DeviceIdentity);
    }
    result
}

pub(crate) fn ready_summary(recovered: bool, cleanup_pending: bool) -> ProfileRecoverySummary {
    ProfileRecoverySummary {
        state: if recovered {
            ProfileRecoveryState::Recovered
        } else {
            ProfileRecoveryState::NotRequired
        },
        can_submit_passphrase: false,
        restart_required: false,
        background_ready: true,
        cleanup_pending,
        losses: Vec::new(),
        admission: None,
    }
}

pub(crate) fn admission_summary(admission: AdmissionRecoverySummary) -> ProfileRecoverySummary {
    ProfileRecoverySummary {
        state: ProfileRecoveryState::AdmissionRecoveryRequired,
        can_submit_passphrase: false,
        restart_required: false,
        background_ready: false,
        cleanup_pending: false,
        losses: Vec::new(),
        admission: Some(admission),
    }
}
