//! 设备信任快照与设备组更新投影。

use super::*;

pub(crate) fn device_trust_snapshot(snapshot: DeviceTrustStatus) -> DeviceTrustSnapshotSummary {
    let impact = |impact: uc_application::facade::DeviceTrustImpact| DeviceTrustImpactSummary {
        usable_device_ids: device_ids(impact.usable_device_ids),
        paused_device_ids: device_ids(impact.paused_device_ids),
        local_device_outcome: device_membership(impact.local_membership),
        requires_rejoin_device_ids: device_ids(impact.requires_rejoin_device_ids),
    };
    let (space_device_update, maintenance_health) =
        super::space_device_update::summaries(snapshot.space_device_update);
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
                apply_impact: impact(change.apply_impact),
                keep_current_impact: impact(change.keep_current_impact),
                allowed_choices: vec![
                    DeviceTrustChoiceSummary::ApplyChange,
                    DeviceTrustChoiceSummary::KeepCurrentDeviceGroup,
                ],
                blocked_reason: None,
            }),
        current_join: snapshot.current_join.map(join_space_status),
        inbound_pairings: snapshot
            .inbound_pairings
            .into_iter()
            .map(|pairing| InboundPairingSummary {
                pairing_id: base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(pairing.pairing_id),
                device_id: pairing.device_id.map(|device_id| device_id.to_string()),
                display_name: pairing.display_name,
                status: match pairing.status {
                    InboundPairingStatus::AwaitingConfirmation => {
                        InboundPairingStatusSummary::AwaitingConfirmation
                    }
                    InboundPairingStatus::ConfirmationMissed => {
                        InboundPairingStatusSummary::ConfirmationMissed
                    }
                    InboundPairingStatus::NeedsAttention => {
                        InboundPairingStatusSummary::NeedsAttention
                    }
                    InboundPairingStatus::Failed => InboundPairingStatusSummary::Failed,
                },
            })
            .collect(),
        pending_inbound_member: snapshot.pending_inbound_member.map(|member| {
            PendingInboundMemberSummary {
                device_id: member.device_id.to_string(),
                display_name: member.display_name,
            }
        }),
        space_device_update,
        maintenance_health,
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
                    DeviceTrustRelationship::ConfirmationPending => {
                        DeviceGroupRelationshipSummary::ConfirmationPending
                    }
                    DeviceTrustRelationship::PendingLocalDecision => {
                        DeviceGroupRelationshipSummary::PendingLocalDecision
                    }
                    DeviceTrustRelationship::AwaitingRemovalAcknowledgement => {
                        DeviceGroupRelationshipSummary::AwaitingRemovalAcknowledgement
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
                pairing_confirmation: device.pairing_confirmation.map(|status| match status {
                    PairingConfirmationStatus::AwaitingPeerConfirmation => {
                        PairingConfirmationSummary::AwaitingPeerConfirmation
                    }
                    PairingConfirmationStatus::Unconfirmed => {
                        PairingConfirmationSummary::Unconfirmed
                    }
                    PairingConfirmationStatus::Confirmed => PairingConfirmationSummary::Confirmed,
                }),
                group_update: device.group_update.map(group_update_summary),
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

fn group_update_summary(update: DeviceGroupUpdate) -> DeviceGroupUpdateSummary {
    let recovery = match update.state.recovery() {
        DeviceGroupUpdateRecovery::Automatic => DeviceGroupUpdateRecoverySummary::Automatic,
        DeviceGroupUpdateRecovery::AutomaticOnReconnect => {
            DeviceGroupUpdateRecoverySummary::AutomaticOnReconnect
        }
        DeviceGroupUpdateRecovery::ReviewDevice => DeviceGroupUpdateRecoverySummary::ReviewDevice,
    };
    DeviceGroupUpdateSummary {
        epoch: update.epoch,
        state: match update.state {
            DeviceGroupUpdateState::Waiting => DeviceGroupUpdateStateSummary::Waiting,
            DeviceGroupUpdateState::ReceiverUnavailable => {
                DeviceGroupUpdateStateSummary::ReceiverUnavailable
            }
            DeviceGroupUpdateState::RejectedUnclassified => {
                DeviceGroupUpdateStateSummary::RejectedUnclassified
            }
            DeviceGroupUpdateState::RejectedByReceiver => {
                DeviceGroupUpdateStateSummary::RejectedByReceiver
            }
        },
        recovery,
        attempts: update.attempts,
        next_retry_at_ms: (recovery == DeviceGroupUpdateRecoverySummary::Automatic)
            .then_some(update.next_attempt_at_ms),
    }
}

pub(super) fn device_sync_relationship(
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

fn device_ids(device_ids: Vec<uc_core::DeviceId>) -> Vec<String> {
    device_ids
        .into_iter()
        .map(|device_id| device_id.to_string())
        .collect()
}

pub(crate) fn device_membership(membership: DeviceTrustMembership) -> DeviceMembershipSummary {
    match membership {
        DeviceTrustMembership::Active => DeviceMembershipSummary::Active,
        DeviceTrustMembership::Removed => DeviceMembershipSummary::Removed,
        DeviceTrustMembership::PendingActivation => DeviceMembershipSummary::Unavailable,
        DeviceTrustMembership::NoCurrentSpace => DeviceMembershipSummary::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff_pending_removal(includes_local_device: bool) -> DeviceTrustStatus {
        use uc_application::deps::SpaceMemberPauseReason;
        use uc_application::facade::{DeviceTrustDevice, PendingDeviceTrustChange};
        use uc_core::{membership::MembershipEventId, DeviceId};

        let members = |ids: &[&str]| {
            ids.iter()
                .map(|id| uc_application::deps::MembershipConflictMember {
                    device: uc_core::membership::MembershipConflictDevice {
                        device_id: DeviceId::new(*id),
                        display_name: (*id).to_owned(),
                    },
                    active: true,
                })
                .collect()
        };

        DeviceTrustStatus {
            revision: 30,
            local_device_id: Some(DeviceId::new("d")),
            local_membership: DeviceTrustMembership::Active,
            current_change: Some(PendingDeviceTrustChange {
                change_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
                proposed_by_device_id: DeviceId::new("b"),
                target_device_ids: vec![DeviceId::new(if includes_local_device {
                    "d"
                } else {
                    "c"
                })],
                includes_local_device,
                explanation: uc_core::membership::MembershipConflictExplanation::unknown(),
                apply_impact: uc_application::facade::DeviceTrustImpact {
                    members: members(&["a", "b", if includes_local_device { "c" } else { "d" }]),
                    pending_confirmation_device_ids: Vec::new(),
                    member_device_ids: ["a", "b", if includes_local_device { "c" } else { "d" }]
                        .map(DeviceId::new)
                        .to_vec(),
                    usable_device_ids: if includes_local_device {
                        Vec::new()
                    } else {
                        ["a", "b", "d"].map(DeviceId::new).to_vec()
                    },
                    paused_device_ids: vec![DeviceId::new(if includes_local_device {
                        "d"
                    } else {
                        "c"
                    })],
                    local_membership: if includes_local_device {
                        DeviceTrustMembership::Removed
                    } else {
                        DeviceTrustMembership::Active
                    },
                    requires_rejoin_device_ids: vec![DeviceId::new(if includes_local_device {
                        "d"
                    } else {
                        "c"
                    })],
                },
                keep_current_impact: uc_application::facade::DeviceTrustImpact {
                    members: members(&["a", "b", "c", "d"]),
                    pending_confirmation_device_ids: Vec::new(),
                    member_device_ids: ["a", "b", "c", "d"].map(DeviceId::new).to_vec(),
                    usable_device_ids: ["a", "c", "d"].map(DeviceId::new).to_vec(),
                    paused_device_ids: vec![DeviceId::new("b")],
                    local_membership: DeviceTrustMembership::Active,
                    requires_rejoin_device_ids: Vec::new(),
                },
            }),
            current_join: None,
            inbound_pairings: Vec::new(),
            pending_inbound_member: None,
            space_device_update: uc_application::facade::SpaceDeviceUpdateStatus::completed(),
            devices: ["a", "b", "c", "d"]
                .into_iter()
                .map(|id| DeviceTrustDevice {
                    device_id: DeviceId::new(id),
                    display_name: id.to_owned(),
                    is_local: id == "d",
                    reachability: ReachabilityState::Offline,
                    membership: DeviceTrustMembership::Active,
                    relationship: if id == "b" {
                        DeviceTrustRelationship::PendingLocalDecision
                    } else if id == "d" {
                        DeviceTrustRelationship::Local
                    } else {
                        DeviceTrustRelationship::Consistent
                    },
                    sync_state: if id == "b" {
                        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::PendingLocalDecision)
                    } else {
                        DeviceTrustSyncState::Usable
                    },
                    pairing_confirmation: None,
                    group_update: None,
                })
                .collect(),
        }
    }

    #[test]
    fn handoff_apply_preview_excludes_the_removed_peer() {
        let summary = device_trust_snapshot(handoff_pending_removal(false));
        let change = summary.current_change.unwrap();
        assert!(
            change
                .apply_impact
                .usable_device_ids
                .iter()
                .all(|id| !change.target_device_ids.contains(id)),
            "接受移除后，继续同步名单仍含移除目标：{:?}",
            change.apply_impact.usable_device_ids
        );
    }

    #[test]
    fn handoff_apply_preview_marks_local_removal() {
        let summary = device_trust_snapshot(handoff_pending_removal(true));
        assert_eq!(
            summary
                .current_change
                .unwrap()
                .apply_impact
                .local_device_outcome,
            DeviceMembershipSummary::Removed
        );
    }

    #[test]
    fn inbound_pairings_are_exposed_separately_from_formal_devices() {
        let mut status = handoff_pending_removal(false);
        status.inbound_pairings = vec![
            uc_application::facade::InboundPairing {
                pairing_id: [0x44; 32],
                device_id: Some(uc_core::DeviceId::new("pending")),
                display_name: Some("Pending".to_owned()),
                status: InboundPairingStatus::ConfirmationMissed,
            },
            uc_application::facade::InboundPairing {
                pairing_id: [0x45; 32],
                device_id: None,
                display_name: None,
                status: InboundPairingStatus::Failed,
            },
        ];

        let summary = device_trust_snapshot(status);

        assert_eq!(summary.inbound_pairings.len(), 2);
        assert_eq!(
            summary.inbound_pairings[0].status,
            InboundPairingStatusSummary::ConfirmationMissed
        );
        assert_eq!(
            summary.inbound_pairings[1].status,
            InboundPairingStatusSummary::Failed
        );
        assert!(summary.inbound_pairings[1].device_id.is_none());
        assert!(summary
            .devices
            .iter()
            .all(|device| device.device_id != "pending"));
    }

    #[test]
    fn pairing_confirmation_is_presentation_only() {
        let mut status = handoff_pending_removal(false);
        let device = status
            .devices
            .iter_mut()
            .find(|device| device.device_id.as_str() == "a")
            .expect("fixture device");
        let original_sync_state = device.sync_state;
        device.pairing_confirmation = Some(PairingConfirmationStatus::Unconfirmed);

        let summary = device_trust_snapshot(status);
        let device = summary
            .devices
            .iter()
            .find(|device| device.device_id == "a")
            .expect("mapped device");
        assert_eq!(
            device.pairing_confirmation,
            Some(PairingConfirmationSummary::Unconfirmed)
        );
        assert_eq!(
            device.sync_relationship,
            device_sync_relationship(original_sync_state, false)
        );
    }

    #[test]
    fn workspace_convergence_errors_have_a_stable_public_mapping() {
        let unavailable = map_roster_error(RosterError::membership_reconciliation_unavailable());
        let corrupt = map_roster_error(RosterError::MembershipReconciliationCorrupt);
        let failed = map_roster_error(RosterError::MemberRemoval(anyhow::anyhow!(
            "internal detail"
        )));
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
}
