//! Shared member roster operations.

use crate::error_codes::*;

use tracing::{error, info};
use uc_application::facade::{
    AppFacade, ContentTypesPatch as AppContentTypesPatch, LegacyBootstrapState,
    LegacyBootstrapView, MemberProtectionStatusView,
    MemberSyncPreferencesPatch as AppMemberSyncPreferencesPatch, MemberSyncPreferencesView,
    MembershipConvergenceFacadeError, RosterError, SpaceProtectionModeView, SpaceProtectionView,
};
use uc_application::membership::{
    MembershipConvergenceError, MembershipConvergenceState,
    SharedDeviceRefreshDeviceState as AppSharedDeviceRefreshDeviceState,
    SharedDeviceRefreshPhase as AppSharedDeviceRefreshPhase, SharedDeviceRefreshStatus,
};
use uc_core::ports::ReachabilityState;

use crate::{
    ContentTypesPatch, ContentTypesSummary, DeviceSummary, EngineError, EngineErrorCategory,
    LegacyBootstrapOutcome, LegacyBootstrapSummary, MemberProtectionStatusSummary,
    MemberProtectionSummary, MemberRemovalPhase, MemberRemovalSummary, MemberSyncPreferencesPatch,
    MemberSyncPreferencesSummary, MembershipConvergenceStateSummary, MembershipConvergenceSummary,
    OperationResult, QueryLegacyBootstrapInput, QueryMemberSyncPreferencesInput,
    QuerySharedDeviceRefreshInput, RemoveMemberInput, SharedDeviceRefreshDeviceStateSummary,
    SharedDeviceRefreshDeviceSummary, SharedDeviceRefreshPhaseSummary,
    SharedDeviceRefreshStartedSummary, SharedDeviceRefreshSummary, SpaceProtectionModeSummary,
    SpaceProtectionSummary, UpdateMemberSyncPreferencesInput,
};

pub async fn execute_list_devices(facade: &AppFacade) -> Result<OperationResult, EngineError> {
    let encryption = facade.encryption_state().await.map_err(|_| {
        error!(
            operation = "list_devices",
            source = "encryption_state",
            error_code = MEMBER_REPOSITORY_FAILED_CODE,
            error_category = "internal",
            retryable = false,
            "device list query failed"
        );
        EngineError::new(
            MEMBER_REPOSITORY_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
        )
    })?;
    if !encryption.initialized {
        info!(
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
    info!(
        operation = "list_devices",
        encryption_initialized = true,
        device_count = devices.len(),
        "device list query completed"
    );
    Ok(OperationResult::Devices(devices))
}

pub async fn execute_query_membership_convergence(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let status = facade
        .membership_convergence()
        .await
        .map_err(|error| match error {
            MembershipConvergenceFacadeError::Unavailable => {
                EngineError::new(1103, EngineErrorCategory::Unavailable, false)
            }
            MembershipConvergenceFacadeError::Query(error) => {
                map_membership_convergence_error(error)
            }
        })?;
    Ok(OperationResult::MembershipConvergence(
        MembershipConvergenceSummary {
            state: match status.state {
                MembershipConvergenceState::Complete => MembershipConvergenceStateSummary::Complete,
                MembershipConvergenceState::Converging => {
                    MembershipConvergenceStateSummary::Converging
                }
                MembershipConvergenceState::WaitingForUpgrade => {
                    MembershipConvergenceStateSummary::WaitingForUpgrade
                }
                MembershipConvergenceState::Blocked => MembershipConvergenceStateSummary::Blocked,
            },
            pending_count: usize_to_u64(status.pending_count),
            waiting_for_peer_count: usize_to_u64(status.waiting_for_peer_count),
            waiting_for_update_count: usize_to_u64(status.waiting_for_update_count),
            version_incompatible_count: usize_to_u64(status.version_incompatible_count),
            blocked_count: usize_to_u64(status.blocked_count),
            rejected_count: usize_to_u64(status.rejected_count),
        },
    ))
}

pub async fn execute_refresh_shared_devices(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let started = facade
        .start_shared_device_refresh()
        .await
        .map_err(|error| match error {
            MembershipConvergenceFacadeError::Unavailable => {
                EngineError::new(1103, EngineErrorCategory::Unavailable, false)
            }
            MembershipConvergenceFacadeError::Query(error) => {
                map_membership_convergence_error(error)
            }
        })?;
    Ok(OperationResult::SharedDeviceRefreshStarted(
        SharedDeviceRefreshStartedSummary {
            request_id: started.request_id,
        },
    ))
}

pub async fn execute_query_shared_device_refresh(
    facade: &AppFacade,
    input: QuerySharedDeviceRefreshInput,
) -> Result<OperationResult, EngineError> {
    Ok(OperationResult::SharedDeviceRefresh(
        facade
            .shared_device_refresh_status(&input.request_id)
            .await
            .map(shared_device_refresh_summary),
    ))
}

pub(crate) fn shared_device_refresh_summary(
    status: SharedDeviceRefreshStatus,
) -> SharedDeviceRefreshSummary {
    SharedDeviceRefreshSummary {
        request_id: status.request_id,
        phase: match status.phase {
            AppSharedDeviceRefreshPhase::Started => SharedDeviceRefreshPhaseSummary::Started,
            AppSharedDeviceRefreshPhase::Discovering => {
                SharedDeviceRefreshPhaseSummary::Discovering
            }
            AppSharedDeviceRefreshPhase::Connecting => SharedDeviceRefreshPhaseSummary::Connecting,
            AppSharedDeviceRefreshPhase::RoundCompleted => {
                SharedDeviceRefreshPhaseSummary::RoundCompleted
            }
        },
        devices: status
            .devices
            .into_iter()
            .map(|device| SharedDeviceRefreshDeviceSummary {
                device_id: device.device_id.as_str().to_owned(),
                display_name: device.device_name,
                state: match device.state {
                    AppSharedDeviceRefreshDeviceState::Discovered => {
                        SharedDeviceRefreshDeviceStateSummary::Discovered
                    }
                    AppSharedDeviceRefreshDeviceState::Connecting => {
                        SharedDeviceRefreshDeviceStateSummary::Connecting
                    }
                    AppSharedDeviceRefreshDeviceState::Connected => {
                        SharedDeviceRefreshDeviceStateSummary::Connected
                    }
                    AppSharedDeviceRefreshDeviceState::AlreadyPresent => {
                        SharedDeviceRefreshDeviceStateSummary::AlreadyPresent
                    }
                    AppSharedDeviceRefreshDeviceState::WaitingForPeer => {
                        SharedDeviceRefreshDeviceStateSummary::WaitingForPeer
                    }
                    AppSharedDeviceRefreshDeviceState::WaitingForUpdate => {
                        SharedDeviceRefreshDeviceStateSummary::WaitingForUpdate
                    }
                    AppSharedDeviceRefreshDeviceState::VersionIncompatible => {
                        SharedDeviceRefreshDeviceStateSummary::VersionIncompatible
                    }
                    AppSharedDeviceRefreshDeviceState::Rejected => {
                        SharedDeviceRefreshDeviceStateSummary::Rejected
                    }
                },
            })
            .collect(),
        total_count: usize_to_u64(status.total_count),
        discovered_count: usize_to_u64(status.discovered_count),
        connecting_count: usize_to_u64(status.connecting_count),
        connected_count: usize_to_u64(status.connected_count),
        already_present_count: usize_to_u64(status.already_present_count),
        waiting_for_peer_count: usize_to_u64(status.waiting_for_peer_count),
        waiting_for_update_count: usize_to_u64(status.waiting_for_update_count),
        version_incompatible_count: usize_to_u64(status.version_incompatible_count),
        rejected_count: usize_to_u64(status.rejected_count),
        unavailable_source_count: usize_to_u64(status.unavailable_source_count),
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn map_membership_convergence_error(error: MembershipConvergenceError) -> EngineError {
    let (code, category, retryable, variant) = match error {
        MembershipConvergenceError::CurrentIdentity(
            uc_core::membership::CurrentMembershipIdentityError::Unavailable,
        ) => (
            QUERY_MEMBERSHIP_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
            "current_identity_unavailable",
        ),
        MembershipConvergenceError::CurrentIdentity(
            uc_core::membership::CurrentMembershipIdentityError::LoadFailed,
        ) => (
            QUERY_MEMBERSHIP_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "current_identity_load",
        ),
        _ => (
            QUERY_MEMBERSHIP_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "membership_state",
        ),
    };
    if category == EngineErrorCategory::Internal {
        error!(variant, "membership convergence query failed");
    }
    EngineError::new(code, category, retryable)
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
    let view = facade
        .remove_member(&input.device_id)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::MemberRemoved(member_removal_summary(view)))
}

pub async fn execute_query_member_removal(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let view = facade
        .member_removal_status()
        .await
        .map_err(map_roster_error)?;
    info!(
        operation = "query_member_removal",
        phase = ?view.phase,
        "member removal status query completed"
    );
    Ok(OperationResult::MemberRemovalStatus(
        member_removal_summary(view),
    ))
}

pub async fn execute_query_space_protection(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let result = facade.space_protection().await.map_err(map_roster_error)?;
    Ok(OperationResult::SpaceProtection(space_protection_summary(
        result,
    )))
}

pub async fn execute_query_legacy_bootstrap(
    facade: &AppFacade,
    input: QueryLegacyBootstrapInput,
) -> Result<OperationResult, EngineError> {
    validate_opaque_id(&input.bootstrap_id)?;
    let result = facade
        .legacy_bootstrap(&input.bootstrap_id)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::LegacyBootstrapStatus(
        result.map(legacy_bootstrap_summary),
    ))
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
    SpaceProtectionSummary {
        mode,
        members,
        legacy_bootstrap: result.legacy_bootstrap.map(legacy_bootstrap_summary),
    }
}

fn legacy_bootstrap_summary(result: LegacyBootstrapView) -> LegacyBootstrapSummary {
    let outcome = match result.state {
        LegacyBootstrapState::AwaitingReadmission => LegacyBootstrapOutcome::AwaitingReadmission,
        LegacyBootstrapState::Complete => LegacyBootstrapOutcome::Complete,
        LegacyBootstrapState::RecoveryRequired => LegacyBootstrapOutcome::RecoveryRequired,
    };
    LegacyBootstrapSummary {
        bootstrap_id: result.bootstrap_id,
        outcome,
        pending_readmission: u64::try_from(result.pending_readmission).unwrap_or(u64::MAX),
    }
}

pub(crate) fn member_removal_summary(
    view: uc_application::facade::MemberRemovalView,
) -> MemberRemovalSummary {
    let phase = match view.phase {
        uc_application::facade::MemberRemovalPhaseView::Applied => MemberRemovalPhase::Applied,
        uc_application::facade::MemberRemovalPhaseView::Converging => {
            MemberRemovalPhase::Converging
        }
        uc_application::facade::MemberRemovalPhaseView::Complete => MemberRemovalPhase::Complete,
        uc_application::facade::MemberRemovalPhaseView::RecoveryRequired => {
            MemberRemovalPhase::RecoveryRequired
        }
    };
    MemberRemovalSummary {
        phase,
        intent_count: u64::try_from(view.intent_count).unwrap_or(u64::MAX),
        effective_member_count: u64::try_from(view.effective_member_count).unwrap_or(u64::MAX),
        convergence_digest: view.convergence_digest,
        updated_at_ms: view.updated_at_ms,
        removed: view.removed,
    }
}

fn validate_opaque_id(opaque_id: &str) -> Result<(), EngineError> {
    if opaque_id.is_empty() || opaque_id.len() > 128 || !opaque_id.is_ascii() {
        return Err(EngineError::new(
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ));
    }
    Ok(())
}

fn validate_device_id(device_id: &str) -> Result<(), EngineError> {
    if device_id.trim().is_empty() {
        return Err(EngineError::new(
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ));
    }
    Ok(())
}

fn into_app_patch(patch: MemberSyncPreferencesPatch) -> AppMemberSyncPreferencesPatch {
    AppMemberSyncPreferencesPatch {
        send_enabled: patch.send_enabled,
        receive_enabled: patch.receive_enabled,
        send_content_types: patch.send_content_types.map(into_app_content_types_patch),
        receive_content_types: patch
            .receive_content_types
            .map(into_app_content_types_patch),
    }
}

fn into_app_content_types_patch(patch: ContentTypesPatch) -> AppContentTypesPatch {
    AppContentTypesPatch {
        text: patch.text,
        image: patch.image,
        link: patch.link,
        file: patch.file,
        code_snippet: patch.code_snippet,
        rich_text: patch.rich_text,
    }
}

fn member_preferences_result(preferences: MemberSyncPreferencesView) -> OperationResult {
    OperationResult::MemberSyncPreferences(MemberSyncPreferencesSummary {
        send_enabled: preferences.send_enabled,
        receive_enabled: preferences.receive_enabled,
        send_content_types: ContentTypesSummary {
            text: preferences.send_content_types.text,
            image: preferences.send_content_types.image,
            link: preferences.send_content_types.link,
            file: preferences.send_content_types.file,
            code_snippet: preferences.send_content_types.code_snippet,
            rich_text: preferences.send_content_types.rich_text,
        },
        receive_content_types: ContentTypesSummary {
            text: preferences.receive_content_types.text,
            image: preferences.receive_content_types.image,
            link: preferences.receive_content_types.link,
            file: preferences.receive_content_types.file,
            code_snippet: preferences.receive_content_types.code_snippet,
            rich_text: preferences.receive_content_types.rich_text,
        },
    })
}

fn map_roster_error(error: RosterError) -> EngineError {
    let (code, category, retryable, variant) = match error {
        RosterError::MemberRemovalUnavailable => (
            1394,
            EngineErrorCategory::Unavailable,
            false,
            "member_removal_unavailable",
        ),
        RosterError::MemberRemoval(_) => (
            1397,
            EngineErrorCategory::InvalidState,
            false,
            "member_removal_failed",
        ),
        RosterError::MemberRemovalInvalidInput => (
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
            "member_removal_invalid_input",
        ),
        RosterError::MemberRemovalTargetNotFound => (
            MEMBER_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
            "member_removal_target_not_found",
        ),
        RosterError::NotFound(_) => (
            MEMBER_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
            "not_found",
        ),
        RosterError::Unavailable => (
            MEMBER_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
            "unavailable",
        ),
        RosterError::MemberRepository(_) => (
            MEMBER_REPOSITORY_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
            "member_repository",
        ),
        RosterError::LocalIdentity(_) => (
            MEMBER_LOCAL_IDENTITY_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
            "local_identity",
        ),
        RosterError::GroupBootstrap(_) => (
            MEMBER_LEGACY_BOOTSTRAP_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "legacy_bootstrap",
        ),
        RosterError::SpaceProtection(_) => (
            SPACE_PROTECTION_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "space_protection",
        ),
    };
    error!(
        operation = "member_roster",
        variant,
        error_code = code,
        error_category = %category,
        retryable,
        "member roster operation failed"
    );
    EngineError::new(code, category, retryable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uc_application::facade::{MemberRemovalPhaseView, MemberRemovalView};

    #[test]
    fn roster_failures_keep_stable_categories_and_distinct_codes() {
        let missing = map_roster_error(RosterError::NotFound("private id".into()));
        let unavailable = map_roster_error(RosterError::Unavailable);
        let repository = map_roster_error(RosterError::MemberRepository("private detail".into()));

        assert_eq!(missing.category(), EngineErrorCategory::NotFound);
        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert_eq!(repository.category(), EngineErrorCategory::Internal);
        assert_ne!(missing.code(), unavailable.code());
        assert_ne!(unavailable.code(), repository.code());
    }

    #[test]
    fn distributed_member_removal_errors_have_a_stable_public_mapping() {
        let unavailable = map_roster_error(RosterError::MemberRemovalUnavailable);
        let failed = map_roster_error(RosterError::MemberRemoval("internal detail".into()));
        let invalid_input = map_roster_error(RosterError::MemberRemovalInvalidInput);
        let target_not_found = map_roster_error(RosterError::MemberRemovalTargetNotFound);

        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert!(!unavailable.is_retryable());
        assert_eq!(failed.category(), EngineErrorCategory::InvalidState);
        assert!(!failed.is_retryable());
        assert_eq!(invalid_input.category(), EngineErrorCategory::InvalidInput);
        assert_eq!(target_not_found.category(), EngineErrorCategory::NotFound);
    }

    #[test]
    fn member_removal_progress_is_preserved_in_the_stable_result() {
        let result = OperationResult::MemberRemoved(member_removal_summary(MemberRemovalView {
            phase: MemberRemovalPhaseView::Applied,
            intent_count: 1,
            effective_member_count: 2,
            convergence_digest: None,
            updated_at_ms: 123,
            removed: false,
        }));

        assert_eq!(
            result,
            OperationResult::MemberRemoved(MemberRemovalSummary {
                phase: MemberRemovalPhase::Applied,
                intent_count: 1,
                effective_member_count: 2,
                convergence_digest: None,
                updated_at_ms: 123,
                removed: false,
            })
        );
    }
}
