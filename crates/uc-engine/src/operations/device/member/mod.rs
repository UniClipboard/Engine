//! 成员名册动作：设备列表、成员同步偏好、移除成员与空间保护状态。

use crate::error_codes::*;

use base64::Engine as _;

use uc_application::facade::{
    AppFacade, ContentTypesPatch as AppContentTypesPatch, CurrentJoinStatus, DeviceGroupUpdate,
    DeviceGroupUpdateRecovery, DeviceGroupUpdateState, DeviceTrustMembership,
    DeviceTrustRelationship, DeviceTrustStatus, DeviceTrustSyncState, InboundPairingStatus,
    JoinSpaceTerminationReason, MemberProtectionStatusView,
    MemberSyncPreferencesPatch as AppMemberSyncPreferencesPatch, MemberSyncPreferencesView,
    PairingConfirmationStatus, RemoveSpaceMemberError, RosterError, SpaceProtectionModeView,
    SpaceProtectionView,
};
use uc_core::ports::ReachabilityState;

use crate::{
    ContentTypesPatch, ContentTypesSummary, DeviceCompatibilitySummary,
    DeviceGroupRelationshipSummary, DeviceGroupUpdateRecoverySummary,
    DeviceGroupUpdateStateSummary, DeviceGroupUpdateSummary, DeviceMembershipSummary,
    DeviceReachabilitySummary, DeviceSummary, DeviceSyncRelationshipSummary,
    DeviceTrustChangeSummary, DeviceTrustChoiceSummary, DeviceTrustImpactSummary,
    DeviceTrustRecoverySummary, DeviceTrustRelationshipSummary, DeviceTrustSnapshotSummary,
    EngineError, EngineErrorCategory, InboundPairingStatusSummary, InboundPairingSummary,
    JoinSpaceAttentionReasonSummary, JoinSpaceAttentionRecoverySummary,
    JoinSpaceRejectionReasonSummary, JoinSpaceStatusSummary, JoinSpaceTerminationReasonSummary,
    JoinedSpaceSummary, MemberProtectionStatusSummary, MemberProtectionSummary,
    MemberSyncPreferencesPatch, MemberSyncPreferencesSummary, OperationResult,
    PairingConfirmationSummary, PendingInboundMemberSummary, QueryMemberSyncPreferencesInput,
    RemoveMemberInput, SpaceProtectionModeSummary, SpaceProtectionSummary,
    UpdateMemberSyncPreferencesInput,
};
use uc_observability_contract::{log_fields::log_vocab, uc_error, uc_info};

mod join;
mod preferences;
mod space_device_update;
mod trust;

pub(crate) use join::join_space_status;
pub(crate) use trust::{device_membership, device_trust_snapshot};

use preferences::{
    into_app_patch, map_roster_error, member_preferences_result, validate_device_id,
};

pub async fn execute_list_devices(facade: &AppFacade) -> Result<OperationResult, EngineError> {
    // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
    let encryption = facade.encryption_state().await.map_err(|_| {
        EngineError::new(
            MEMBER_REPOSITORY_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
        )
    })?;
    if !encryption.initialized {
        uc_info!(
            operation = "list_devices",
            encryption_initialized = false,
            device_count = 0,
            "device list query completed"
        );
        return Ok(OperationResult::Devices(Vec::new()));
    }
    let entries = facade
        .list_roster_entries()
        .await
        .map_err(map_roster_error)?;
    let devices = entries
        .into_iter()
        .map(|entry| DeviceSummary {
            device_id: entry.device_id.as_str().to_string(),
            display_name: entry.device_name,
            is_local: entry.is_local,
            online: entry.is_local || entry.state == ReachabilityState::Online,
        })
        .collect::<Vec<_>>();
    uc_info!(
        operation = "list_devices",
        encryption_initialized = true,
        device_count = devices.len(),
        "device list query completed"
    );
    Ok(OperationResult::Devices(devices))
}

pub async fn execute_query_member_sync_preferences(
    facade: &AppFacade,
    input: QueryMemberSyncPreferencesInput,
) -> Result<OperationResult, EngineError> {
    validate_device_id(&input.device_id)?;
    let preferences = facade
        .member_sync_preferences(&input.device_id)
        .await
        .map_err(map_roster_error)?;
    Ok(member_preferences_result(preferences))
}

pub async fn execute_update_member_sync_preferences(
    facade: &AppFacade,
    input: UpdateMemberSyncPreferencesInput,
) -> Result<OperationResult, EngineError> {
    validate_device_id(&input.device_id)?;
    let preferences = facade
        .update_member_sync_preferences(&input.device_id, into_app_patch(input.patch))
        .await
        .map_err(map_roster_error)?;
    Ok(member_preferences_result(preferences))
}

pub async fn execute_remove_member(
    facade: &AppFacade,
    input: RemoveMemberInput,
) -> Result<OperationResult, EngineError> {
    validate_device_id(&input.device_id)?;
    let result = facade
        .remove_space_member(&uc_core::DeviceId::new(input.device_id))
        .await
        .map_err(map_remove_space_member_error)?;
    Ok(OperationResult::DeviceTrust(device_trust_snapshot(
        result.status,
    )))
}

pub async fn execute_query_space_protection(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let result = facade.space_protection().await.map_err(map_roster_error)?;
    Ok(OperationResult::SpaceProtection(space_protection_summary(
        result,
    )))
}

fn space_protection_summary(result: SpaceProtectionView) -> SpaceProtectionSummary {
    let mode = match result.mode {
        SpaceProtectionModeView::Legacy => SpaceProtectionModeSummary::Legacy,
        SpaceProtectionModeView::Migrating => SpaceProtectionModeSummary::Migrating,
        SpaceProtectionModeView::Ready => SpaceProtectionModeSummary::Ready,
    };
    let members = result
        .members
        .into_iter()
        .map(|member| MemberProtectionSummary {
            device_id: member.device_id,
            status: match member.status {
                MemberProtectionStatusView::LegacyUnprotected => {
                    MemberProtectionStatusSummary::LegacyUnprotected
                }
                MemberProtectionStatusView::Protected => MemberProtectionStatusSummary::Protected,
                MemberProtectionStatusView::AwaitingReadmission => {
                    MemberProtectionStatusSummary::AwaitingReadmission
                }
                MemberProtectionStatusView::RequiresReadmission => {
                    MemberProtectionStatusSummary::RequiresReadmission
                }
                MemberProtectionStatusView::RecoveryRequired => {
                    MemberProtectionStatusSummary::RecoveryRequired
                }
            },
        })
        .collect();
    SpaceProtectionSummary { mode, members }
}

fn map_remove_space_member_error(error: RemoveSpaceMemberError) -> EngineError {
    // 结果记录由 RemoveSpaceMemberUseCase 负责；这里只做稳定错误码映射。
    let (code, category, retryable) = match error {
        RemoveSpaceMemberError::Locked => (
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
        ),
        // 成员状态或本机签名暂时不可用（例如加入后仍在切换 Space 会话）：稍后重试即可完成。
        RemoveSpaceMemberError::Unavailable => (
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        RemoveSpaceMemberError::RecoveryRequired { .. } => (
            QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        RemoveSpaceMemberError::StateChanged => (
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            true,
        ),
        RemoveSpaceMemberError::TargetNotFound => {
            (MEMBER_NOT_FOUND_CODE, EngineErrorCategory::NotFound, false)
        }
        RemoveSpaceMemberError::SelfTarget => (
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        RemoveSpaceMemberError::LocalMemberRemoved => (
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        // 本机已提交移除，但其他成员尚未确认；与 StateChanged 的错误码相同，二者由完成记录的 error_class 区分。
        RemoveSpaceMemberError::CommittedButPending { .. } => (
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            true,
        ),
    };
    EngineError::new(code, category, retryable)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remove_member_mapping_does_not_write_a_second_record() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();

        for error in [
            RemoveSpaceMemberError::Unavailable,
            RemoveSpaceMemberError::StateChanged,
            RemoveSpaceMemberError::recovery_required(),
            RemoveSpaceMemberError::LocalMemberRemoved,
            RemoveSpaceMemberError::SelfTarget,
        ] {
            map_remove_space_member_error(error);
        }

        assert_eq!(logs.count("member removal"), 0, "{}", logs.output());
    }

    #[test]
    fn temporarily_unavailable_removal_is_retryable_but_a_locked_space_is_not() {
        let unavailable = map_remove_space_member_error(RemoveSpaceMemberError::Unavailable);
        let locked = map_remove_space_member_error(RemoveSpaceMemberError::Locked);

        assert_eq!(
            unavailable.code(),
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE
        );
        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert!(unavailable.is_retryable());
        assert_eq!(locked.code(), QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE);
        assert!(!locked.is_retryable());
    }

    #[test]
    fn concurrent_member_change_is_retryable_instead_of_reported_as_corruption() {
        let error = map_remove_space_member_error(RemoveSpaceMemberError::StateChanged);

        assert_eq!(error.code(), QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE);
        assert_eq!(error.category(), EngineErrorCategory::InvalidState);
        assert!(error.is_retryable());
    }
}
