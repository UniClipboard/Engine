//! Shared member roster operations.

use crate::error_codes::*;

use tracing::{error, info};
use uc_application::facade::{
    AppFacade, ContentTypesPatch as AppContentTypesPatch, LegacyBootstrapState,
    LegacyBootstrapView, MemberProtectionStatusView, MemberRevocationState, MemberRevocationView,
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
    ContentTypesPatch, ContentTypesSummary, ContinueMemberRevocationInput, DeviceSummary,
    EngineError, EngineErrorCategory, LegacyBootstrapOutcome, LegacyBootstrapSummary,
    MemberProtectionStatusSummary, MemberProtectionSummary, MemberRevocationOutcome,
    MemberRevocationSummary, MemberSyncPreferencesPatch, MemberSyncPreferencesSummary,
    MembershipConvergenceStateSummary, MembershipConvergenceSummary, OperationResult,
    QueryLegacyBootstrapInput, QueryMemberRevocationInput, QueryMemberSyncPreferencesInput,
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
    let result = facade
        .remove_member(&input.device_id)
        .await
        .map_err(map_roster_error)?;
    Ok(member_revocation_result(result))
}

pub async fn execute_secure_remove_legacy_member(
    facade: &AppFacade,
    input: RemoveMemberInput,
) -> Result<OperationResult, EngineError> {
    validate_device_id(&input.device_id)?;
    let result = facade
        .secure_remove_legacy_member(&input.device_id)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::LegacyMemberRemoval(
        legacy_bootstrap_summary(result),
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

pub async fn execute_query_member_revocation(
    facade: &AppFacade,
    input: QueryMemberRevocationInput,
) -> Result<OperationResult, EngineError> {
    validate_opaque_id(&input.revocation_id)?;
    let result = facade
        .member_revocation(&input.revocation_id)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::MemberRevocationStatus(
        result.map(member_revocation_summary),
    ))
}

pub async fn execute_query_current_member_revocation(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .current_member_revocation()
        .await
        .map_err(map_roster_error)?;
    let result = result.map(member_revocation_summary);
    info!(
        operation = "query_current_member_revocation",
        has_member_revocation = result.is_some(),
        "current member revocation query completed"
    );
    Ok(OperationResult::MemberRevocationStatus(result))
}

pub async fn execute_continue_member_revocation(
    facade: &AppFacade,
    input: ContinueMemberRevocationInput,
) -> Result<OperationResult, EngineError> {
    validate_opaque_id(&input.revocation_id)?;
    if input.permanently_lost_device_ids.is_empty() {
        return Err(EngineError::new(
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ));
    }
    for device_id in &input.permanently_lost_device_ids {
        validate_device_id(device_id)?;
    }
    let result = facade
        .continue_member_revocation(&input.revocation_id, &input.permanently_lost_device_ids)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::MemberRevocationStatus(Some(
        member_revocation_summary(result),
    )))
}

fn member_revocation_result(result: MemberRevocationView) -> OperationResult {
    OperationResult::MemberRemoved(member_revocation_summary(result))
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

pub(crate) fn member_revocation_summary(result: MemberRevocationView) -> MemberRevocationSummary {
    let outcome = match result.state {
        MemberRevocationState::LocalOnly => MemberRevocationOutcome::LocalOnly,
        MemberRevocationState::Applied => MemberRevocationOutcome::Applied,
        MemberRevocationState::Complete => MemberRevocationOutcome::Complete,
        MemberRevocationState::RecoveryRequired => MemberRevocationOutcome::RecoveryRequired,
    };
    MemberRevocationSummary {
        revocation_id: result.revocation_id,
        outcome,
        pending_recipients: u64::try_from(result.pending_recipient_device_ids.len())
            .unwrap_or(u64::MAX),
        removed_device_ids: result.removed_device_ids,
        pending_recipient_device_ids: result.pending_recipient_device_ids,
        updated_at_ms: result.updated_at_ms,
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
        RosterError::NotFound(_) => (
            MEMBER_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
            "not_found",
        ),
        RosterError::LocalDeviceRemoval => (
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
            "local_device_removal",
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
        RosterError::PeerAddressRepository(_) => (
            MEMBER_PEER_ADDRESS_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
            "peer_address_repository",
        ),
        RosterError::TrustedPeerRepository(_) => (
            MEMBER_TRUSTED_PEER_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
            "trusted_peer_repository",
        ),
        RosterError::GroupRevocation(_) => (
            MEMBER_GROUP_REVOCATION_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "group_revocation",
        ),
        RosterError::MemberRemovalInProgress => (
            MEMBER_REMOVAL_IN_PROGRESS_CODE,
            EngineErrorCategory::Conflict,
            true,
            "member_removal_in_progress",
        ),
        RosterError::MemberRemovalRecoveryRequired => (
            MEMBER_REMOVAL_RECOVERY_REQUIRED_CODE,
            EngineErrorCategory::InvalidState,
            false,
            "member_removal_recovery_required",
        ),
        RosterError::InvalidPermanentLossSelection => (
            MEMBER_INVALID_PERMANENT_LOSS_SELECTION_CODE,
            EngineErrorCategory::InvalidInput,
            false,
            "invalid_permanent_loss_selection",
        ),
        RosterError::LegacyBootstrapRequired => (
            MEMBER_LEGACY_BOOTSTRAP_REQUIRED_CODE,
            EngineErrorCategory::InvalidState,
            false,
            "legacy_bootstrap_required",
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
        RosterError::LocalMemberUnavailable => (
            MEMBER_LOCAL_MEMBER_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
            "local_member_unavailable",
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
    use uc_application::facade::{MemberRevocationState, MemberRevocationView};

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
    fn active_member_removal_conflicts_have_stable_public_errors() {
        let in_progress = map_roster_error(RosterError::MemberRemovalInProgress);
        let recovery_required = map_roster_error(RosterError::MemberRemovalRecoveryRequired);

        assert_eq!(in_progress.code(), MEMBER_REMOVAL_IN_PROGRESS_CODE);
        assert_eq!(in_progress.category(), EngineErrorCategory::Conflict);
        assert!(in_progress.is_retryable());
        assert_eq!(
            recovery_required.code(),
            MEMBER_REMOVAL_RECOVERY_REQUIRED_CODE
        );
        assert_eq!(
            recovery_required.category(),
            EngineErrorCategory::InvalidState
        );
        assert!(!recovery_required.is_retryable());
        let invalid_selection = map_roster_error(RosterError::InvalidPermanentLossSelection);
        assert_eq!(
            invalid_selection.code(),
            MEMBER_INVALID_PERMANENT_LOSS_SELECTION_CODE
        );
        assert_eq!(
            invalid_selection.category(),
            EngineErrorCategory::InvalidInput
        );
        assert!(!invalid_selection.is_retryable());
    }

    #[test]
    fn member_revocation_progress_is_preserved_in_the_stable_result() {
        let result = member_revocation_result(MemberRevocationView {
            revocation_id: Some("revocation-a".into()),
            state: MemberRevocationState::Applied,
            pending_recipients: 2,
            removed_device_ids: vec!["dev-removed".into()],
            pending_recipient_device_ids: vec!["dev-c".into(), "dev-d".into()],
            updated_at_ms: 123,
        });

        assert_eq!(
            result,
            OperationResult::MemberRemoved(MemberRevocationSummary {
                revocation_id: Some("revocation-a".into()),
                outcome: MemberRevocationOutcome::Applied,
                pending_recipients: 2,
                removed_device_ids: vec!["dev-removed".into()],
                pending_recipient_device_ids: vec!["dev-c".into(), "dev-d".into()],
                updated_at_ms: 123,
            })
        );
    }
}
