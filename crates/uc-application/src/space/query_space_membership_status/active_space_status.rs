use std::collections::BTreeMap;
use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, MembershipHistoryRelationship, MembershipOperationV2, SpaceMember,
    SpaceMembershipState, VersionedMembershipHistory,
};
use uc_core::ports::{PresencePort, ReachabilityState};

use super::{
    ActionUnavailableReason, ActiveSpaceStatusResult, DeviceCompatibility, DeviceMembership,
    GroupRelationship, PendingSpaceMembershipChange, RecoveryAvailability, SpaceMemberRelationship,
    SpaceMembershipAction, SpaceMembershipChangeChoice, SpaceMembershipChangeImpact,
    SpaceMembershipStatus, SyncRelationship,
};

pub(crate) struct ActiveSpaceStatusFacts {
    pub(crate) state: SpaceMembershipState,
    pub(crate) history: VersionedMembershipHistory,
    pub(crate) own_instance: MemberInstanceId,
    pub(crate) local_membership: DeviceMembership,
    pub(crate) roster: Vec<SpaceMember>,
    pub(crate) presence: Arc<dyn PresencePort>,
    pub(crate) local_device_id: DeviceId,
}

/// 根据已验证的成员事实生成活动 Space 的产品状态。
pub(crate) async fn build_active_space_status(
    facts: ActiveSpaceStatusFacts,
) -> ActiveSpaceStatusResult {
    let ActiveSpaceStatusFacts {
        state,
        history,
        own_instance,
        local_membership,
        roster,
        presence,
        local_device_id,
    } = facts;
    let workspace_unverifiable = state.failure_category.is_some();
    let mut candidate_devices = roster
        .iter()
        .map(|member| member.device_id.clone())
        .collect::<Vec<_>>();
    candidate_devices.push(local_device_id.clone());
    candidate_devices.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    candidate_devices.dedup();
    let pending_facts = if workspace_unverifiable {
        None
    } else {
        history
            .pending_removal_decision(own_instance)
            .and_then(|removal_event_id| {
                let event = history.event(removal_event_id)?;
                let MembershipOperationV2::RemoveDevice { member } = event.operation else {
                    return None;
                };
                let proposed_by_device_id = history
                    .device_for_member(&event.author_member_instance_id, &candidate_devices)?;
                let target_device_id = history.device_for_member(&member, &candidate_devices)?;
                Some(uc_core::membership::PendingRemovalFacts::new(
                    removal_event_id,
                    proposed_by_device_id,
                    vec![target_device_id],
                    [member].into(),
                ))
            })
    };
    let includes_local_device = pending_facts
        .as_ref()
        .is_some_and(|facts| facts.includes_member(own_instance));

    let mut names = BTreeMap::new();
    for member in roster {
        names.insert(member.device_id, member.device_name);
    }
    names.entry(local_device_id.clone()).or_default();

    let mut devices = Vec::with_capacity(names.len());
    for (device_id, display_name) in names {
        let is_local = device_id == local_device_id;
        let reachability = if is_local {
            ReachabilityState::Online
        } else {
            presence.current_state(&device_id).await
        };
        let membership = if is_local {
            local_membership
        } else if history.effective_members().iter().any(|member| {
            history
                .device_for_member(member, &candidate_devices)
                .as_ref()
                == Some(&device_id)
        }) {
            DeviceMembership::Active
        } else if history.has_admitted_device(&device_id, &candidate_devices) {
            DeviceMembership::Removed
        } else {
            DeviceMembership::Unknown
        };
        let relationship = state.peer_history_relationships.get(&device_id);
        let group_relationship = if workspace_unverifiable {
            GroupRelationship::Unverifiable
        } else {
            match relationship {
                Some(MembershipHistoryRelationship::Consistent) => GroupRelationship::Consistent,
                Some(MembershipHistoryRelationship::PendingRemovalDecision) => {
                    GroupRelationship::PendingLocalDecision
                }
                Some(MembershipHistoryRelationship::Diverged) => GroupRelationship::Diverged,
                Some(MembershipHistoryRelationship::Invalid) => GroupRelationship::Unverifiable,
                Some(MembershipHistoryRelationship::Unknown)
                | Some(MembershipHistoryRelationship::UpgradeRequired)
                | None => GroupRelationship::Unknown,
            }
        };
        let compatibility = match relationship {
            Some(MembershipHistoryRelationship::UpgradeRequired) => {
                DeviceCompatibility::UpgradeRequired
            }
            Some(MembershipHistoryRelationship::Invalid)
            | Some(MembershipHistoryRelationship::Unknown)
            | None
                if !is_local =>
            {
                DeviceCompatibility::Unknown
            }
            _ => DeviceCompatibility::Compatible,
        };
        let sync_relationship = if local_membership == DeviceMembership::Removed {
            SyncRelationship::RemovedLocalDevice
        } else if membership == DeviceMembership::Removed {
            SyncRelationship::RemovedPeerDevice
        } else {
            match (group_relationship, compatibility) {
                (GroupRelationship::Unverifiable, _) => SyncRelationship::PausedUnverifiable,
                (GroupRelationship::PendingLocalDecision, _) => {
                    SyncRelationship::WaitingForLocalDecision
                }
                (GroupRelationship::Diverged, _) => SyncRelationship::PausedGroupDiverged,
                (_, DeviceCompatibility::UpgradeRequired) => {
                    SyncRelationship::PausedUpgradeRequired
                }
                (GroupRelationship::Consistent, DeviceCompatibility::Compatible) => {
                    SyncRelationship::Usable
                }
                _ if is_local => SyncRelationship::Usable,
                _ => SyncRelationship::Unknown,
            }
        };
        let (available_actions, blocked_reason) = match sync_relationship {
            SyncRelationship::RemovedLocalDevice => (
                vec![SpaceMembershipAction::RejoinDeviceGroup],
                Some(ActionUnavailableReason::LocalDeviceRemoved),
            ),
            SyncRelationship::PausedUpgradeRequired if is_local => {
                (vec![SpaceMembershipAction::UpdateThisDevice], None)
            }
            SyncRelationship::PausedUpgradeRequired => (
                Vec::new(),
                Some(ActionUnavailableReason::PeerUpgradeRequired),
            ),
            SyncRelationship::PausedUnverifiable => (
                Vec::new(),
                Some(ActionUnavailableReason::DeviceFactsUnverifiable),
            ),
            _ => (Vec::new(), None),
        };
        devices.push(SpaceMemberRelationship {
            device_id,
            display_name,
            is_local,
            reachability,
            membership,
            group_relationship,
            compatibility,
            sync_relationship,
            available_actions,
            blocked_reason,
        });
    }

    let current_change = pending_facts.map(|facts| {
        let mut apply_usable = devices
            .iter()
            .filter(|device| {
                device.membership == DeviceMembership::Active
                    && !facts.target_device_ids.contains(&device.device_id)
            })
            .map(|device| device.device_id.clone())
            .collect::<Vec<_>>();
        apply_usable.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        let mut keep_usable = devices
            .iter()
            .filter(|device| device.membership == DeviceMembership::Active)
            .map(|device| device.device_id.clone())
            .collect::<Vec<_>>();
        keep_usable.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        let mut paused = vec![facts.proposed_by_device_id.clone()];
        paused.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        PendingSpaceMembershipChange {
            change_id: facts.removal_event_id,
            proposed_by_device_id: facts.proposed_by_device_id,
            target_device_ids: facts.target_device_ids.clone(),
            includes_local_device,
            apply_impact: SpaceMembershipChangeImpact {
                usable_device_ids: apply_usable,
                paused_device_ids: Vec::new(),
                local_device_outcome: if includes_local_device {
                    DeviceMembership::Removed
                } else {
                    local_membership
                },
                requires_rejoin_device_ids: facts.target_device_ids,
            },
            keep_current_impact: SpaceMembershipChangeImpact {
                usable_device_ids: keep_usable,
                paused_device_ids: paused,
                local_device_outcome: local_membership,
                requires_rejoin_device_ids: Vec::new(),
            },
            allowed_choices: vec![
                SpaceMembershipChangeChoice::ApplyChange,
                SpaceMembershipChangeChoice::KeepCurrentDeviceGroup,
            ],
            blocked_reason: includes_local_device
                .then_some(ActionUnavailableReason::LocalDeviceConfirmationRequired),
        }
    });
    let (allowed_actions, blocked_reason) = if workspace_unverifiable {
        (
            Vec::new(),
            Some(ActionUnavailableReason::DeviceFactsUnverifiable),
        )
    } else {
        match &current_change {
            Some(change) if change.includes_local_device => (
                vec![
                    SpaceMembershipAction::KeepCurrentDeviceGroup,
                    SpaceMembershipAction::ConfirmApplyRemovesLocalDevice,
                ],
                Some(ActionUnavailableReason::LocalDeviceConfirmationRequired),
            ),
            Some(_) => (
                vec![
                    SpaceMembershipAction::ApplyCurrentChange,
                    SpaceMembershipAction::KeepCurrentDeviceGroup,
                ],
                None,
            ),
            None if local_membership == DeviceMembership::Removed => (
                vec![SpaceMembershipAction::RejoinDeviceGroup],
                Some(ActionUnavailableReason::LocalDeviceRemoved),
            ),
            None => (Vec::new(), Some(ActionUnavailableReason::NoCurrentChange)),
        }
    };

    ActiveSpaceStatusResult {
        space_lineage: state.space_lineage.clone(),
        status: SpaceMembershipStatus {
            revision: state.revision,
            local_device_id,
            local_membership,
            current_change,
            current_join: None,
            pending_inbound_member: None,
            devices,
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions,
            blocked_reason,
            updated_at_ms: state.updated_at_ms,
        },
    }
}
