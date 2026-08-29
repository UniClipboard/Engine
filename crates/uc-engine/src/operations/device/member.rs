//! Shared member roster operations.

use crate::error_codes::*;

use base64::Engine as _;
use tracing::{error, info};
use uc_application::facade::{
    AppFacade, ContentTypesPatch as AppContentTypesPatch, CurrentJoinStatus,
    DecideDeviceTrustChange, DecideDeviceTrustChangeError, DecideDeviceTrustChangeResult,
    DeviceTrustChangeChoice, DeviceTrustMembership, DeviceTrustRelationship, DeviceTrustStatus,
    DeviceTrustSyncState, MemberProtectionStatusView,
    MemberSyncPreferencesPatch as AppMemberSyncPreferencesPatch, MemberSyncPreferencesView,
    QueryDeviceTrustError, RemoveSpaceMemberError, RosterError, SpaceProtectionModeView,
    SpaceProtectionView,
};
#[cfg(any(test, feature = "dev-tools"))]
use uc_core::membership::WorkspaceSnapshot;
use uc_core::ports::ReachabilityState;

use crate::{
    ContentTypesPatch, ContentTypesSummary, DecideDeviceTrustChangeInput,
    DeviceCompatibilitySummary, DeviceGroupRelationshipSummary, DeviceMembershipSummary,
    DeviceReachabilitySummary, DeviceSummary, DeviceSyncRelationshipSummary,
    DeviceTrustChangeSummary, DeviceTrustChoiceSummary, DeviceTrustDecisionSummary,
    DeviceTrustImpactSummary, DeviceTrustRecoverySummary, DeviceTrustRelationshipSummary,
    DeviceTrustSnapshotSummary, EngineError, EngineErrorCategory, JoinSpaceRejectionReasonSummary,
    JoinSpaceStatusSummary, JoinedSpaceSummary, MemberProtectionStatusSummary,
    MemberProtectionSummary, MemberSyncPreferencesPatch, MemberSyncPreferencesSummary,
    OperationResult, PendingInboundMemberSummary, QueryMemberSyncPreferencesInput,
    RemoveMemberInput, SpaceProtectionModeSummary, SpaceProtectionSummary,
    UpdateMemberSyncPreferencesInput,
};
#[cfg(feature = "dev-tools")]
use crate::{DecideMembershipRemovalInput, MembershipRemovalDecision};
#[cfg(any(test, feature = "dev-tools"))]
use crate::{
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

#[cfg(feature = "dev-tools")]
pub async fn execute_query_workspace_convergence(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let snapshot = facade
        .workspace_convergence()
        .await
        .map_err(map_roster_error)?;
    Ok(OperationResult::WorkspaceMembership(
        workspace_convergence_summary(snapshot),
    ))
}

pub async fn execute_query_space_membership_status(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let snapshot = facade
        .query_device_trust()
        .await
        .map_err(map_query_device_trust_error)?;
    Ok(OperationResult::DeviceTrust(device_trust_snapshot(
        snapshot,
    )))
}

pub async fn execute_decide_device_trust_change(
    facade: &AppFacade,
    input: DecideDeviceTrustChangeInput,
) -> Result<OperationResult, EngineError> {
    let change_id =
        uc_core::membership::MembershipEventId::from_hex(&input.change_id).ok_or_else(|| {
            EngineError::new(
                MEMBER_INVALID_INPUT_CODE,
                EngineErrorCategory::InvalidInput,
                false,
            )
        })?;
    let decision = match input.choice {
        DeviceTrustChoiceSummary::ApplyChange => DeviceTrustChangeChoice::ApplyChange,
        DeviceTrustChoiceSummary::KeepCurrentDeviceGroup => {
            DeviceTrustChangeChoice::KeepCurrentDeviceGroup
        }
    };
    let result = facade
        .decide_device_trust_change(DecideDeviceTrustChange {
            change_id,
            choice: decision,
            confirm_local_removal: input.confirm_local_removal,
        })
        .await
        .map_err(map_decide_device_trust_change_error)?;
    Ok(OperationResult::DeviceTrustDecision(device_trust_decision(
        result,
    )))
}

fn device_trust_decision(result: DecideDeviceTrustChangeResult) -> DeviceTrustDecisionSummary {
    match result {
        DecideDeviceTrustChangeResult::Applied { change_id, status } => {
            DeviceTrustDecisionSummary::Applied {
                change_id: change_id.to_hex(),
                snapshot: Box::new(device_trust_snapshot(status)),
            }
        }
        DecideDeviceTrustChangeResult::KeptCurrentDeviceGroup { change_id, status } => {
            DeviceTrustDecisionSummary::KeptCurrentDeviceGroup {
                change_id: change_id.to_hex(),
                snapshot: Box::new(device_trust_snapshot(status)),
            }
        }
        DecideDeviceTrustChangeResult::AlreadyCompleted {
            change_id,
            choice,
            status,
        } => DeviceTrustDecisionSummary::AlreadyCompleted {
            change_id: change_id.to_hex(),
            completed_choice: device_trust_choice(choice),
            snapshot: Box::new(device_trust_snapshot(status)),
        },
        DecideDeviceTrustChangeResult::StateChanged {
            current_change_id,
            status,
        } => DeviceTrustDecisionSummary::StateChanged {
            current_change_id: current_change_id.map(|change_id| change_id.to_hex()),
            snapshot: Box::new(device_trust_snapshot(status)),
        },
        DecideDeviceTrustChangeResult::LocalConfirmationRequired { change_id, status } => {
            DeviceTrustDecisionSummary::LocalDeviceConfirmationRequired {
                change_id: change_id.to_hex(),
                snapshot: Box::new(device_trust_snapshot(status)),
            }
        }
    }
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

#[cfg(feature = "dev-tools")]
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
    Ok(OperationResult::WorkspaceMembership(
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

#[cfg(any(test, feature = "dev-tools"))]
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

pub(crate) fn device_trust_snapshot(snapshot: DeviceTrustStatus) -> DeviceTrustSnapshotSummary {
    let usable_device_ids = snapshot
        .devices
        .iter()
        .filter(|device| matches!(device.sync_state, DeviceTrustSyncState::Usable))
        .map(|device| device.device_id.to_string())
        .collect::<Vec<_>>();
    let paused_device_ids = snapshot
        .devices
        .iter()
        .filter(|device| matches!(device.sync_state, DeviceTrustSyncState::Paused(_)))
        .map(|device| device.device_id.to_string())
        .collect::<Vec<_>>();
    let current_impact = DeviceTrustImpactSummary {
        usable_device_ids,
        paused_device_ids,
        local_device_outcome: device_membership(snapshot.local_membership),
        requires_rejoin_device_ids: Vec::new(),
    };
    DeviceTrustSnapshotSummary {
        revision: snapshot.revision,
        local_device_id: snapshot
            .local_device_id
            .map(|device_id| device_id.to_string())
            .unwrap_or_default(),
        local_membership: device_membership(snapshot.local_membership),
        current_change: snapshot
            .current_change
            .map(|change| DeviceTrustChangeSummary {
                change_id: change.change_id.to_hex(),
                proposed_by_device_id: change.proposed_by_device_id.to_string(),
                target_device_ids: device_ids(change.target_device_ids),
                includes_local_device: change.includes_local_device,
                apply_impact: current_impact.clone(),
                keep_current_impact: current_impact,
                allowed_choices: vec![
                    DeviceTrustChoiceSummary::ApplyChange,
                    DeviceTrustChoiceSummary::KeepCurrentDeviceGroup,
                ],
                blocked_reason: None,
            }),
        current_join: snapshot.current_join.map(join_space_status),
        pending_inbound_member: snapshot.pending_inbound_member.map(|member| {
            PendingInboundMemberSummary {
                device_id: member.device_id.to_string(),
                display_name: member.display_name,
            }
        }),
        devices: snapshot
            .devices
            .into_iter()
            .map(|device| DeviceTrustRelationshipSummary {
                device_id: device.device_id.to_string(),
                display_name: device.display_name,
                is_local: device.is_local,
                reachability: match device.reachability {
                    ReachabilityState::Online => DeviceReachabilitySummary::Online,
                    ReachabilityState::Offline => DeviceReachabilitySummary::Offline,
                    ReachabilityState::Unknown => DeviceReachabilitySummary::Unknown,
                },
                membership: device_membership(device.membership),
                group_relationship: match device.relationship {
                    DeviceTrustRelationship::Local | DeviceTrustRelationship::Consistent => {
                        DeviceGroupRelationshipSummary::Consistent
                    }
                    DeviceTrustRelationship::PendingLocalDecision => {
                        DeviceGroupRelationshipSummary::PendingLocalDecision
                    }
                    DeviceTrustRelationship::Diverged => DeviceGroupRelationshipSummary::Diverged,
                    DeviceTrustRelationship::Invalid => {
                        DeviceGroupRelationshipSummary::Unverifiable
                    }
                    DeviceTrustRelationship::UpgradeRequired | DeviceTrustRelationship::Unknown => {
                        DeviceGroupRelationshipSummary::Unknown
                    }
                },
                compatibility: match device.relationship {
                    DeviceTrustRelationship::UpgradeRequired => {
                        DeviceCompatibilitySummary::UpgradeRequired
                    }
                    DeviceTrustRelationship::Unknown => DeviceCompatibilitySummary::Unknown,
                    _ => DeviceCompatibilitySummary::Compatible,
                },
                sync_relationship: device_sync_relationship(device.sync_state, device.is_local),
                available_actions: Vec::new(),
                blocked_reason: None,
            })
            .collect(),
        recovery: DeviceTrustRecoverySummary::NotAvailableInThisVersion,
        allowed_actions: Vec::new(),
        blocked_reason: None,
        updated_at_ms: 0,
    }
}

fn device_sync_relationship(
    state: DeviceTrustSyncState,
    is_local: bool,
) -> DeviceSyncRelationshipSummary {
    use uc_application::deps::SpaceMemberPauseReason;

    match state {
        DeviceTrustSyncState::Usable => DeviceSyncRelationshipSummary::Usable,
        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::PendingLocalDecision) => {
            DeviceSyncRelationshipSummary::WaitingForLocalDecision
        }
        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::Diverged) => {
            DeviceSyncRelationshipSummary::PausedGroupDiverged
        }
        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::UpgradeRequired) => {
            DeviceSyncRelationshipSummary::PausedUpgradeRequired
        }
        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::LocalMemberInactive) if is_local => {
            DeviceSyncRelationshipSummary::RemovedLocalDevice
        }
        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::LocalMemberInactive) => {
            DeviceSyncRelationshipSummary::RemovedPeerDevice
        }
        DeviceTrustSyncState::Paused(
            SpaceMemberPauseReason::Invalid
            | SpaceMemberPauseReason::RelationshipUnconfirmed
            | SpaceMemberPauseReason::EffectPending,
        ) => DeviceSyncRelationshipSummary::PausedUnverifiable,
    }
}

pub(crate) fn join_space_status(status: CurrentJoinStatus) -> JoinSpaceStatusSummary {
    match status {
        CurrentJoinStatus::Active {
            join_id,
            joined_space,
        } => JoinSpaceStatusSummary::Active {
            join_id: encode_join_id(join_id),
            joined_space: JoinedSpaceSummary {
                sponsor_device_id: joined_space.sponsor_device_id.to_string(),
                sponsor_identity_fingerprint: joined_space
                    .sponsor_identity_fingerprint
                    .as_display()
                    .to_string(),
                space_id: joined_space.space_id,
                self_device_id: joined_space.self_device_id.to_string(),
                self_identity_fingerprint: joined_space
                    .self_identity_fingerprint
                    .as_display()
                    .to_string(),
                migrated_records: joined_space.migrated_records,
                preserved_unreadable_records: joined_space.preserved_unreadable_records,
            },
        },
        CurrentJoinStatus::Pending {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            cancel_requested,
        } => JoinSpaceStatusSummary::Pending {
            join_id: encode_join_id(join_id),
            target_space_id,
            sponsor_device_id: sponsor_device_id.map(|device_id| device_id.to_string()),
            sponsor_identity_fingerprint: sponsor_identity_fingerprint
                .map(|fingerprint| fingerprint.as_display().to_string()),
            cancel_requested,
        },
        CurrentJoinStatus::Rejected { join_id, reason } => JoinSpaceStatusSummary::Rejected {
            join_id: encode_join_id(join_id),
            reason: match reason {
                uc_core::membership::AdmissionRejectionReason::InvitationUnavailable => {
                    JoinSpaceRejectionReasonSummary::InvitationUnavailable
                }
                uc_core::membership::AdmissionRejectionReason::AuthenticationRejected => {
                    JoinSpaceRejectionReasonSummary::AuthenticationRejected
                }
                uc_core::membership::AdmissionRejectionReason::IdentityConflict => {
                    JoinSpaceRejectionReasonSummary::IdentityConflict
                }
                uc_core::membership::AdmissionRejectionReason::BaseHistoryChanged => {
                    JoinSpaceRejectionReasonSummary::BaseHistoryChanged
                }
                uc_core::membership::AdmissionRejectionReason::JoinerHistoryAhead => {
                    JoinSpaceRejectionReasonSummary::JoinerHistoryAhead
                }
                uc_core::membership::AdmissionRejectionReason::HistoryConflict => {
                    JoinSpaceRejectionReasonSummary::HistoryConflict
                }
                uc_core::membership::AdmissionRejectionReason::PeerUpgradeRequired => {
                    JoinSpaceRejectionReasonSummary::PeerUpgradeRequired
                }
                uc_core::membership::AdmissionRejectionReason::Cancelled => {
                    JoinSpaceRejectionReasonSummary::Cancelled
                }
                uc_core::membership::AdmissionRejectionReason::RemovedBeforeActivation => {
                    JoinSpaceRejectionReasonSummary::RemovedBeforeActivation
                }
            },
        },
    }
}

fn encode_join_id(join_id: [u8; 16]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(join_id)
}

fn device_ids(device_ids: Vec<uc_core::DeviceId>) -> Vec<String> {
    device_ids
        .into_iter()
        .map(|device_id| device_id.to_string())
        .collect()
}

fn device_membership(membership: DeviceTrustMembership) -> DeviceMembershipSummary {
    match membership {
        DeviceTrustMembership::Active => DeviceMembershipSummary::Active,
        DeviceTrustMembership::Removed => DeviceMembershipSummary::Removed,
        DeviceTrustMembership::PendingActivation => DeviceMembershipSummary::Unavailable,
        DeviceTrustMembership::NoCurrentSpace => DeviceMembershipSummary::Unknown,
    }
}

fn device_trust_choice(choice: DeviceTrustChangeChoice) -> DeviceTrustChoiceSummary {
    match choice {
        DeviceTrustChangeChoice::ApplyChange => DeviceTrustChoiceSummary::ApplyChange,
        DeviceTrustChangeChoice::KeepCurrentDeviceGroup => {
            DeviceTrustChoiceSummary::KeepCurrentDeviceGroup
        }
    }
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
        RosterError::MembershipReconciliationLocked => (
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
            "workspace_convergence_locked",
        ),
        RosterError::MembershipReconciliationCorrupt => (
            QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE,
            EngineErrorCategory::InvalidState,
            false,
            "workspace_convergence_corrupt",
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

fn map_query_device_trust_error(error: QueryDeviceTrustError) -> EngineError {
    match error {
        QueryDeviceTrustError::Locked | QueryDeviceTrustError::Unavailable => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
        ),
        QueryDeviceTrustError::RecoveryRequired => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        QueryDeviceTrustError::Dependency { .. } => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
    }
}

fn map_decide_device_trust_change_error(error: DecideDeviceTrustChangeError) -> EngineError {
    match error {
        DecideDeviceTrustChangeError::Locked | DecideDeviceTrustChangeError::Unavailable => {
            EngineError::new(
                QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                false,
            )
        }
        DecideDeviceTrustChangeError::RecoveryRequired
        | DecideDeviceTrustChangeError::StateChanged => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        DecideDeviceTrustChangeError::CommittedButPending => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            true,
        ),
    }
}

fn map_remove_space_member_error(error: RemoveSpaceMemberError) -> EngineError {
    match error {
        RemoveSpaceMemberError::Locked | RemoveSpaceMemberError::Unavailable => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
        ),
        RemoveSpaceMemberError::RecoveryRequired | RemoveSpaceMemberError::StateChanged => {
            EngineError::new(
                QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE,
                EngineErrorCategory::InvalidState,
                false,
            )
        }
        RemoveSpaceMemberError::TargetNotFound => {
            EngineError::new(MEMBER_NOT_FOUND_CODE, EngineErrorCategory::NotFound, false)
        }
        RemoveSpaceMemberError::SelfTarget => EngineError::new(
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        RemoveSpaceMemberError::LocalMemberRemoved => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        RemoveSpaceMemberError::CommittedButPending { .. } => EngineError::new(
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            true,
        ),
    }
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
        let corrupt = map_roster_error(RosterError::MembershipReconciliationCorrupt);
        let failed = map_roster_error(RosterError::MemberRemoval("internal detail".into()));
        let invalid_input = map_roster_error(RosterError::MemberRemovalInvalidInput);
        let target_not_found = map_roster_error(RosterError::MemberRemovalTargetNotFound);

        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert!(!unavailable.is_retryable());
        assert_eq!(corrupt.category(), EngineErrorCategory::InvalidState);
        assert_eq!(corrupt.code(), QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE);
        assert_ne!(corrupt.code(), failed.code());
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
