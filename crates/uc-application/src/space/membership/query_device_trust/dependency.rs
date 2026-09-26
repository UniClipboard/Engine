//! 设备信任查询的外部依赖失败诊断。
//!
//! 查询对调用方只返回 `Dependency`；运行诊断另记是哪一项依赖失败及其固定原因分类，不记录错误正文、
//! 设备标识或其他资料。

use uc_core::membership::{KeyEpochError, KeyEpochStateIssue};

use super::QueryDeviceTrustError;

/// 设备信任查询读取的外部依赖。
#[derive(Debug, Clone, Copy)]
pub(super) enum TrustDependency {
    AdmissionDisplay,
    SecurityUpdateStatus,
    DeviceObservations,
    LocalIdentity,
}

impl TrustDependency {
    fn as_str(self) -> &'static str {
        match self {
            Self::AdmissionDisplay => "admission_display",
            Self::SecurityUpdateStatus => "security_update_status",
            Self::DeviceObservations => "device_observations",
            Self::LocalIdentity => "local_identity",
        }
    }

    /// 依赖失败时记录一条运行诊断，错误原样返回。
    pub(super) fn diagnose(self, error: QueryDeviceTrustError) -> QueryDeviceTrustError {
        if let QueryDeviceTrustError::Dependency { source } = &error {
            tracing::warn!(
                dependency = self.as_str(),
                cause = cause_of(source),
                "设备信任查询的依赖失败"
            );
        }
        error
    }
}

/// 原因的固定分类：只识别组密钥状态错误，其余依赖记为 `other`。
fn cause_of(source: &anyhow::Error) -> &'static str {
    let Some(error) = source
        .chain()
        .find_map(|error| error.downcast_ref::<KeyEpochError>())
    else {
        return "other";
    };
    match error {
        KeyEpochError::Repository(_) => "key_epoch_repository",
        KeyEpochError::SecurityState { .. } => "key_epoch_security_state",
        KeyEpochError::SpaceNotReady => "key_epoch_space_not_ready",
        KeyEpochError::StateIssue(issue) => match issue {
            KeyEpochStateIssue::MissingMaterial => "key_epoch_missing_material",
            KeyEpochStateIssue::CorruptMaterial => "key_epoch_corrupt_material",
            KeyEpochStateIssue::EpochMismatch => "key_epoch_epoch_mismatch",
            KeyEpochStateIssue::MissingRevocation => "key_epoch_missing_revocation",
            KeyEpochStateIssue::MissingStage => "key_epoch_missing_stage",
            KeyEpochStateIssue::RecoveryRequired => "key_epoch_recovery_required",
            KeyEpochStateIssue::UnsupportedUpdate => "key_epoch_unsupported_update",
            KeyEpochStateIssue::OutOfOrderUpdate => "key_epoch_out_of_order_update",
            KeyEpochStateIssue::UnsupportedOperation => "key_epoch_unsupported_operation",
            KeyEpochStateIssue::InvalidStage => "key_epoch_invalid_stage",
            KeyEpochStateIssue::StateChanged => "key_epoch_state_changed",
        },
        _ => "key_epoch_other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_epoch_causes_are_classified_without_error_text() {
        let state = QueryDeviceTrustError::Dependency {
            source: anyhow::Error::new(KeyEpochError::StateIssue(
                KeyEpochStateIssue::MissingMaterial,
            )),
        };
        let QueryDeviceTrustError::Dependency { source } =
            TrustDependency::SecurityUpdateStatus.diagnose(state)
        else {
            panic!("dependency error must be returned unchanged");
        };
        assert_eq!(cause_of(&source), "key_epoch_missing_material");
        assert_eq!(cause_of(&anyhow::anyhow!("private detail")), "other");
    }
}
