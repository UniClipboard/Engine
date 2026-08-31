use std::collections::BTreeMap;
use std::sync::Arc;

use uc_core::membership::{
    MemberInstanceId, MembershipHistoryRelationship, MembershipOperationV2,
    VersionedMembershipHistory,
};
use uc_core::ports::ReachabilityState;

use crate::space::membership::MembershipLedger;
use crate::space::membership::SpaceMemberPauseReason;

use super::{
    DeviceTrustDevice, DeviceTrustMembership, DeviceTrustObservation, DeviceTrustRelationship,
    DeviceTrustStatus, DeviceTrustSyncState, LoadCurrentJoinStatusPort,
    LoadDeviceTrustObservationsPort, PendingDeviceTrustChange, QueryDeviceTrustError,
};

pub(crate) struct QueryDeviceTrustUseCase {
    ledger: Arc<MembershipLedger>,
    observations: Arc<dyn LoadDeviceTrustObservationsPort>,
    current_join: Arc<dyn LoadCurrentJoinStatusPort>,
}

impl QueryDeviceTrustUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        observations: Arc<dyn LoadDeviceTrustObservationsPort>,
        current_join: Arc<dyn LoadCurrentJoinStatusPort>,
    ) -> Self {
        Self {
            ledger,
            observations,
            current_join,
        }
    }

    pub(crate) async fn execute(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError> {
        let snapshot = self.ledger.load_verified().await?;
        let current_join = self.current_join.load_current_join().await?;
        if snapshot.history().is_none() {
            let mut status = DeviceTrustStatus::no_current_space(snapshot.record().revision);
            status.current_join = current_join;
            return Ok(status);
        }
        let history = snapshot
            .history()
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let scope = snapshot
            .current_scope()
            .map_err(|_| QueryDeviceTrustError::RecoveryRequired)?;
        let local_device_id = snapshot
            .record()
            .local_device_id
            .clone()
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
        let local_member_instance = snapshot
            .record()
            .local_member_instance
            .ok_or(QueryDeviceTrustError::RecoveryRequired)?;

        let mut device_ids = history
            .active_members()
            .into_iter()
            .map(|member| {
                history
                    .admission_facts_for(member)
                    .map(|facts| facts.device_id.clone())
                    .ok_or(QueryDeviceTrustError::RecoveryRequired)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !device_ids.contains(&local_device_id) {
            device_ids.push(local_device_id.clone());
        }
        device_ids.extend(snapshot.record().peer_reconciliation.keys().cloned());
        device_ids.sort();
        device_ids.dedup();

        let observations = self.observations.load(&device_ids).await?;
        let mut observations_by_device = BTreeMap::new();
        for observation in observations {
            if !device_ids.contains(&observation.device_id)
                || observations_by_device
                    .insert(observation.device_id.clone(), observation)
                    .is_some()
            {
                return Err(QueryDeviceTrustError::RecoveryRequired);
            }
        }
        let mut paused_by_device = scope
            .paused_peer_devices
            .iter()
            .map(|paused| (paused.device_id.clone(), paused.reason))
            .collect::<BTreeMap<_, _>>();
        let mut devices = Vec::with_capacity(device_ids.len());
        for device_id in &device_ids {
            let is_local = device_id == &local_device_id;
            let member = if is_local {
                Some(local_member_instance)
            } else {
                history.member_for_device(device_id, &device_ids)
            };
            let facts = member.and_then(|member| history.admission_facts_for(member));
            let observation = match observations_by_device.remove(device_id) {
                Some(observation) => observation,
                None if member.is_none_or(|member| !history.active_members().contains(&member)) => {
                    DeviceTrustObservation {
                        device_id: device_id.clone(),
                        display_name: None,
                        reachability: ReachabilityState::Offline,
                    }
                }
                None => return Err(QueryDeviceTrustError::Unavailable),
            };
            let membership = if is_local {
                if scope.local_member_active {
                    DeviceTrustMembership::Active
                } else if history.active_members().contains(&local_member_instance) {
                    DeviceTrustMembership::PendingActivation
                } else {
                    DeviceTrustMembership::Removed
                }
            } else if member.is_some_and(|member| history.active_members().contains(&member)) {
                DeviceTrustMembership::Active
            } else {
                DeviceTrustMembership::Removed
            };
            let relationship = if is_local {
                DeviceTrustRelationship::Local
            } else {
                snapshot
                    .record()
                    .peer_reconciliation
                    .get(device_id)
                    .map(|record| map_relationship(record.relationship))
                    .unwrap_or(DeviceTrustRelationship::Unknown)
            };
            let sync_state = if membership == DeviceTrustMembership::Removed {
                DeviceTrustSyncState::Paused(SpaceMemberPauseReason::LocalMemberInactive)
            } else if is_local || scope.usable_peer_device_ids.contains(device_id) {
                DeviceTrustSyncState::Usable
            } else {
                DeviceTrustSyncState::Paused(
                    paused_by_device
                        .remove(device_id)
                        .or_else(|| {
                            snapshot
                                .record()
                                .peer_reconciliation
                                .get(device_id)
                                .and_then(|record| pause_reason(record.relationship))
                        })
                        .ok_or(QueryDeviceTrustError::RecoveryRequired)?,
                )
            };
            devices.push(DeviceTrustDevice {
                device_id: device_id.clone(),
                display_name: observation
                    .display_name
                    .or_else(|| facts.map(|facts| facts.device_name.clone()))
                    .unwrap_or_else(|| device_id.as_str().to_owned()),
                is_local,
                reachability: observation.reachability,
                membership,
                relationship,
                sync_state,
            });
        }

        let current_change = pending_change(history, local_member_instance)?;
        Ok(DeviceTrustStatus {
            revision: snapshot.record().revision,
            local_device_id: Some(local_device_id),
            local_membership: if scope.local_member_active {
                DeviceTrustMembership::Active
            } else if history.active_members().contains(&local_member_instance) {
                DeviceTrustMembership::PendingActivation
            } else {
                DeviceTrustMembership::Removed
            },
            current_change,
            current_join,
            pending_inbound_member: None,
            devices,
        })
    }
}

fn pending_change(
    history: &VersionedMembershipHistory,
    local_member: MemberInstanceId,
) -> Result<Option<PendingDeviceTrustChange>, QueryDeviceTrustError> {
    let Some(change_id) = history.pending_removal_decision(local_member) else {
        return Ok(None);
    };
    let event = history
        .event(change_id)
        .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
    let target = match &event.operation {
        MembershipOperationV2::RemoveDevice { member } => *member,
        MembershipOperationV2::AddDevice { .. } => {
            return Err(QueryDeviceTrustError::RecoveryRequired);
        }
    };
    let proposed_by_device_id = history
        .admission_facts_for(event.author_member_instance_id)
        .map(|facts| facts.device_id.clone())
        .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
    let target_device_id = history
        .admission_facts_for(target)
        .map(|facts| facts.device_id.clone())
        .ok_or(QueryDeviceTrustError::RecoveryRequired)?;
    Ok(Some(PendingDeviceTrustChange {
        change_id,
        proposed_by_device_id,
        target_device_ids: vec![target_device_id],
        includes_local_device: target == local_member,
    }))
}

fn map_relationship(relationship: MembershipHistoryRelationship) -> DeviceTrustRelationship {
    match relationship {
        MembershipHistoryRelationship::Unknown => DeviceTrustRelationship::Unknown,
        MembershipHistoryRelationship::Consistent => DeviceTrustRelationship::Consistent,
        MembershipHistoryRelationship::UpgradeRequired => DeviceTrustRelationship::UpgradeRequired,
        MembershipHistoryRelationship::PendingRemovalDecision => {
            DeviceTrustRelationship::PendingLocalDecision
        }
        MembershipHistoryRelationship::Diverged => DeviceTrustRelationship::Diverged,
        MembershipHistoryRelationship::Invalid => DeviceTrustRelationship::Invalid,
    }
}

fn pause_reason(
    relationship: MembershipHistoryRelationship,
) -> Option<crate::space::membership::SpaceMemberPauseReason> {
    use crate::space::membership::SpaceMemberPauseReason;

    match relationship {
        MembershipHistoryRelationship::PendingRemovalDecision => {
            Some(SpaceMemberPauseReason::PendingLocalDecision)
        }
        MembershipHistoryRelationship::Diverged => Some(SpaceMemberPauseReason::Diverged),
        MembershipHistoryRelationship::Invalid => Some(SpaceMemberPauseReason::Invalid),
        MembershipHistoryRelationship::UpgradeRequired => {
            Some(SpaceMemberPauseReason::UpgradeRequired)
        }
        MembershipHistoryRelationship::Unknown => {
            Some(SpaceMemberPauseReason::RelationshipUnconfirmed)
        }
        MembershipHistoryRelationship::Consistent => None,
    }
}
