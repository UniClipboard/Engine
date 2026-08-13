//! Shared member roster operations.

use crate::error_codes::*;

use tracing::{error, info};
use uc_application::facade::{
    AppFacade, ContentTypesPatch as AppContentTypesPatch, LegacyBootstrapState,
    LegacyBootstrapView, MemberProtectionStatusView,
    MemberSyncPreferencesPatch as AppMemberSyncPreferencesPatch, MemberSyncPreferencesView,
    RosterError, SpaceProtectionModeView, SpaceProtectionView,
};
use uc_core::membership::WorkspaceSnapshot;
use uc_core::ports::ReachabilityState;

use crate::{
    ContentTypesPatch, ContentTypesSummary, DecideMembershipRemovalInput, DeviceSummary,
    EngineError, EngineErrorCategory, LegacyBootstrapOutcome, LegacyBootstrapSummary,
    MemberProtectionStatusSummary, MemberProtectionSummary, MemberSyncPreferencesPatch,
    MemberSyncPreferencesSummary, MembershipRemovalDecision, OperationResult,
    QueryLegacyBootstrapInput, QueryMemberSyncPreferencesInput, RemoveMemberInput,
    SpaceProtectionModeSummary, SpaceProtectionSummary, UpdateMemberSyncPreferencesInput,
    WorkspaceConvergenceFailureCategorySummary, WorkspaceConvergencePhaseSummary,
    WorkspaceConvergenceSummary,
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

pub async fn execute_query_workspace_convergence(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let snapshot = facade
        .workspace_convergence()
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::WorkspaceConvergence(
        workspace_convergence_summary(snapshot),
    ))
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
    let snapshot = facade
        .remove_member(&input.device_id)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::WorkspaceConvergence(
        workspace_convergence_summary(snapshot),
    ))
}

pub async fn execute_decide_membership_removal(
    facade: &AppFacade,
    input: DecideMembershipRemovalInput,
) -> Result<OperationResult, EngineError> {
    let removal_event_id = uc_core::membership::MembershipEventId::from_hex(
        &input.removal_event_id,
    )
    .ok_or_else(|| {
        EngineError::new(
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        )
    })?;
    let decision = match input.decision {
        MembershipRemovalDecision::Accept => uc_core::membership::RemovalDecision::Accept,
        MembershipRemovalDecision::Reject => uc_core::membership::RemovalDecision::Reject,
    };
    let snapshot = facade
        .decide_membership_removal(removal_event_id, decision)
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::WorkspaceConvergence(
        workspace_convergence_summary(snapshot),
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

pub(crate) fn workspace_convergence_summary(
    snapshot: WorkspaceSnapshot,
) -> WorkspaceConvergenceSummary {
    WorkspaceConvergenceSummary {
        phase: match snapshot.phase {
            uc_core::membership::WorkspacePhase::LocallyApplied => {
                WorkspaceConvergencePhaseSummary::LocallyApplied
            }
            uc_core::membership::WorkspacePhase::Converging => {
                WorkspaceConvergencePhaseSummary::Converging
            }
            uc_core::membership::WorkspacePhase::Complete => {
                WorkspaceConvergencePhaseSummary::Complete
            }
            uc_core::membership::WorkspacePhase::RecoveryRequired => {
                WorkspaceConvergencePhaseSummary::RecoveryRequired
            }
        },
        revision: snapshot.revision,
        history_event_count: u64::try_from(snapshot.history_event_count).unwrap_or(u64::MAX),
        effective_member_count: u64::try_from(snapshot.effective_member_count).unwrap_or(u64::MAX),
        pending_removal_decision_device_ids: snapshot
            .pending_removal_decision_device_ids
            .into_iter()
            .map(|device_id| device_id.to_string())
            .collect(),
        pending_removal_decision_event_id: snapshot
            .pending_removal_decision_event_id
            .map(|event_id| event_id.to_hex()),
        diverged_peer_device_ids: snapshot
            .diverged_peer_device_ids
            .into_iter()
            .map(|device_id| device_id.to_string())
            .collect(),
        upgrade_required_peer_device_ids: snapshot
            .upgrade_required_peer_device_ids
            .into_iter()
            .map(|device_id| device_id.to_string())
            .collect(),
        convergence_digest: snapshot.convergence_digest.map(|digest| digest.to_string()),
        removed: snapshot.removed,
        updated_at_ms: snapshot.updated_at_ms,
        failure_category: snapshot.failure_category.map(|category| match category {
            uc_core::membership::WorkspaceFailureCategory::SpaceMismatch => {
                WorkspaceConvergenceFailureCategorySummary::SpaceMismatch
            }
            uc_core::membership::WorkspaceFailureCategory::ContinuityGap => {
                WorkspaceConvergenceFailureCategorySummary::ContinuityGap
            }
            uc_core::membership::WorkspaceFailureCategory::IdentityMismatch => {
                WorkspaceConvergenceFailureCategorySummary::IdentityMismatch
            }
            uc_core::membership::WorkspaceFailureCategory::DigestConflict => {
                WorkspaceConvergenceFailureCategorySummary::DigestConflict
            }
            uc_core::membership::WorkspaceFailureCategory::Unauthorized => {
                WorkspaceConvergenceFailureCategorySummary::Unauthorized
            }
            uc_core::membership::WorkspaceFailureCategory::VersionIncompatible => {
                WorkspaceConvergenceFailureCategorySummary::VersionIncompatible
            }
            uc_core::membership::WorkspaceFailureCategory::NoEffectiveMembers => {
                WorkspaceConvergenceFailureCategorySummary::NoEffectiveMembers
            }
            uc_core::membership::WorkspaceFailureCategory::Storage => {
                WorkspaceConvergenceFailureCategorySummary::Storage
            }
        }),
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
        RosterError::MembershipReconciliationUnavailable => (
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
            "workspace_convergence_unavailable",
        ),
        RosterError::MemberRemoval(_) => (
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            false,
            "workspace_convergence_failed",
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
    use uc_core::membership::{WorkspaceFailureCategory, WorkspacePhase};

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
    fn workspace_convergence_errors_have_a_stable_public_mapping() {
        let unavailable = map_roster_error(RosterError::MembershipReconciliationUnavailable);
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
    fn workspace_convergence_snapshot_is_preserved_in_the_stable_result() {
        let summary = workspace_convergence_summary(WorkspaceSnapshot {
            phase: WorkspacePhase::LocallyApplied,
            revision: 3,
            history_event_count: 1,
            effective_member_count: 2,
            pending_removal_decision_device_ids: vec![uc_core::ids::DeviceId::new("device-c")],
            pending_removal_decision_event_id: Some(
                uc_core::membership::MembershipEventId::from_hex(
                    "0101010101010101010101010101010101010101010101010101010101010101",
                )
                .unwrap(),
            ),
            diverged_peer_device_ids: vec![uc_core::ids::DeviceId::new("device-d")],
            upgrade_required_peer_device_ids: vec![uc_core::ids::DeviceId::new("device-e")],
            convergence_digest: None,
            removed: false,
            updated_at_ms: 123,
            failure_category: Some(WorkspaceFailureCategory::Storage),
        });

        assert_eq!(
            summary,
            WorkspaceConvergenceSummary {
                phase: WorkspaceConvergencePhaseSummary::LocallyApplied,
                revision: 3,
                history_event_count: 1,
                effective_member_count: 2,
                pending_removal_decision_device_ids: vec!["device-c".to_owned()],
                pending_removal_decision_event_id: Some(
                    "0101010101010101010101010101010101010101010101010101010101010101".to_owned(),
                ),
                diverged_peer_device_ids: vec!["device-d".to_owned()],
                upgrade_required_peer_device_ids: vec!["device-e".to_owned()],
                convergence_digest: None,
                removed: false,
                updated_at_ms: 123,
                failure_category: Some(WorkspaceConvergenceFailureCategorySummary::Storage),
            }
        );
    }
}
