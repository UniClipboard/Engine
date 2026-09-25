use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use uc_core::membership::{
    LedgerMemberStatus, LedgerUpdateProblem, LedgerUpdateView, MemberInstanceId, MembershipLedger,
    MembershipOperationV2, PeerLink, PeerRelationView, PeerSyncView, SecurityDeliveryStatus,
    VersionedMembershipHistory,
};
use uc_core::ports::{LocalIdentityPort, ReachabilityState};
use uc_observability_contract::diagnostics::connectivity::{
    record_local_identity_changed, LocalIdentityState,
};

use crate::space::membership::{
    pause_reason, MembershipOwner, MembershipView, SpaceMemberPauseReason,
};

use super::dependency::TrustDependency;
use super::{
    DeviceTrustDevice, DeviceTrustImpact, DeviceTrustMembership, DeviceTrustObservation,
    DeviceTrustRelationship, DeviceTrustStatus, DeviceTrustSyncState, LoadCurrentJoinStatusPort,
    LoadDeviceTrustObservationsPort, LoadSecurityDeviceUpdateStatusPort, PairingConfirmationTarget,
    PendingDeviceTrustChange, QueryDeviceTrustError, SpaceDeviceUpdatePhase,
    SpaceDeviceUpdateProblem, SpaceDeviceUpdateRecovery, SpaceDeviceUpdateStatus,
};

/// 设备信任状态只从成员账本的 `present` 得出；本查询只叠加展示资料、组密钥投递观察和本机身份核对。
pub(crate) struct QueryDeviceTrustUseCase {
    owner: Arc<MembershipOwner>,
    observations: Arc<dyn LoadDeviceTrustObservationsPort>,
    current_join: Arc<dyn LoadCurrentJoinStatusPort>,
    security_updates: Arc<dyn LoadSecurityDeviceUpdateStatusPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    last_identity_mismatch: Mutex<Option<bool>>,
}

impl QueryDeviceTrustUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        observations: Arc<dyn LoadDeviceTrustObservationsPort>,
        current_join: Arc<dyn LoadCurrentJoinStatusPort>,
        security_updates: Arc<dyn LoadSecurityDeviceUpdateStatusPort>,
        local_identity: Arc<dyn LocalIdentityPort>,
    ) -> Self {
        Self {
            owner,
            observations,
            current_join,
            security_updates,
            local_identity,
            last_identity_mismatch: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_tests(
        owner: Arc<MembershipOwner>,
        observations: Arc<dyn LoadDeviceTrustObservationsPort>,
        current_join: Arc<dyn LoadCurrentJoinStatusPort>,
    ) -> Self {
        Self::new(
            owner,
            observations,
            current_join,
            Arc::new(CompletedSecurityUpdates),
            Arc::new(MissingLocalIdentity),
        )
    }

    pub(crate) async fn execute(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError> {
        let view = self.owner.load().await?;
        let (status, mismatch) = self.query_view_with_identity(&view).await?;
        let latest = self.owner.load().await?;
        if latest.revision() == view.revision() {
            self.record_identity_change(mismatch);
            return Ok(status);
        }
        let (status, mismatch) = self.query_view_with_identity(&latest).await?;
        self.record_identity_change(mismatch);
        Ok(status)
    }

    pub(crate) async fn query_view(
        &self,
        view: &MembershipView,
    ) -> Result<DeviceTrustStatus, QueryDeviceTrustError> {
        let (status, mismatch) = self.query_view_with_identity(view).await?;
        self.record_identity_change(mismatch);
        Ok(status)
    }

    async fn query_view_with_identity(
        &self,
        view: &MembershipView,
    ) -> Result<(DeviceTrustStatus, Option<bool>), QueryDeviceTrustError> {
        let Some(space) = view.space() else {
            let mut status = DeviceTrustStatus::no_current_space(view.revision());
            status.current_join = self
                .current_join
                .load_admission_display(&[])
                .await
                .map_err(|error| TrustDependency::AdmissionDisplay.diagnose(error))?
                .current_join;
            return Ok((status, None));
        };
        let ledger = space.ledger();
        let history = space.history();
        let local_device_id = *space.local_device_id();
        let local_member_instance = space.local_member();
        let confirmation_targets = history
            .active_members()
            .into_iter()
            .filter(|member| *member != local_member_instance)
            .filter_map(|member_instance_id| {
                history
                    .admission_event_id_for(member_instance_id)
                    .map(|add_event_id| PairingConfirmationTarget {
                        member_instance_id,
                        add_event_id,
                    })
            })
            .collect::<Vec<_>>();
        let admission_display = self
            .current_join
            .load_admission_display(&confirmation_targets)
            .await
            .map_err(|error| TrustDependency::AdmissionDisplay.diagnose(error))?;
        let current_join = admission_display.current_join;
        let inbound_pairings = admission_display.inbound_pairings;
        let pending_inbound_member = admission_display.pending_inbound_member;
        let mut pairing_confirmations = BTreeMap::new();
        for observation in admission_display.pairing_confirmations {
            if !confirmation_targets.contains(&observation.target)
                || pairing_confirmations
                    .insert(observation.target, observation.status)
                    .is_some()
            {
                return Err(QueryDeviceTrustError::recovery_required());
            }
        }
        let security_updates = self
            .security_updates
            .load_security_device_update_status()
            .await
            .map_err(|error| TrustDependency::SecurityUpdateStatus.diagnose(error))?;
        let presented = ledger
            .present(security_delivery(security_updates))
            .map_err(QueryDeviceTrustError::recovery_required_from)?;
        let device_ids: Vec<_> = presented
            .devices
            .iter()
            .map(|device| device.device_id)
            .collect();
        let observations = self
            .observations
            .load(&device_ids)
            .await
            .map_err(|error| TrustDependency::DeviceObservations.diagnose(error))?;
        let mut observations_by_device = BTreeMap::new();
        for observation in observations {
            if !device_ids.contains(&observation.device_id)
                || observations_by_device
                    .insert(observation.device_id, observation)
                    .is_some()
            {
                return Err(QueryDeviceTrustError::recovery_required());
            }
        }
        let mut devices = Vec::with_capacity(presented.devices.len());
        for device in &presented.devices {
            let facts = device
                .member
                .and_then(|member| history.admission_facts_for(member));
            let membership = membership_of(device.status);
            let observation = match observations_by_device.remove(&device.device_id) {
                Some(observation) => observation,
                None if membership != DeviceTrustMembership::Active => DeviceTrustObservation {
                    device_id: device.device_id,
                    display_name: None,
                    reachability: ReachabilityState::Offline,
                },
                None => return Err(QueryDeviceTrustError::Unavailable),
            };
            devices.push(DeviceTrustDevice {
                device_id: device.device_id,
                display_name: observation
                    .display_name
                    .or_else(|| facts.map(|facts| facts.device_name.clone()))
                    .unwrap_or_else(|| device.device_id.as_str().to_owned()),
                is_local: device.is_local,
                reachability: observation.reachability,
                membership,
                relationship: relationship_of(device.relation),
                sync_state: match device.sync {
                    PeerSyncView::Usable => DeviceTrustSyncState::Usable,
                    PeerSyncView::Paused(reason) => {
                        DeviceTrustSyncState::Paused(pause_reason(reason))
                    }
                },
                pairing_confirmation: device
                    .member
                    .and_then(|member_instance_id| {
                        history
                            .admission_event_id_for(member_instance_id)
                            .map(|add_event_id| PairingConfirmationTarget {
                                member_instance_id,
                                add_event_id,
                            })
                    })
                    .and_then(|target| pairing_confirmations.get(&target).copied()),
            });
        }

        let current_change = pending_change(history, local_member_instance, &devices, ledger)?;
        let local_membership = membership_of(presented.local_status);
        let mut space_device_update = match presented.device_update {
            LedgerUpdateView::Updating => SpaceDeviceUpdateStatus::updating(),
            LedgerUpdateView::Completed => SpaceDeviceUpdateStatus::completed(),
            LedgerUpdateView::RetryableFailure { next_retry_at_ms } => {
                SpaceDeviceUpdateStatus::retryable_failure(next_retry_at_ms)
            }
            LedgerUpdateView::NeedsAttention(problem) => match problem {
                LedgerUpdateProblem::DeviceStateRejected => {
                    SpaceDeviceUpdateStatus::needs_attention(
                        SpaceDeviceUpdateProblem::DeviceStateRejected,
                        SpaceDeviceUpdateRecovery::ReviewDevices,
                    )
                }
                LedgerUpdateProblem::DeviceRelationshipConflict => {
                    SpaceDeviceUpdateStatus::needs_attention(
                        SpaceDeviceUpdateProblem::DeviceRelationshipConflict,
                        SpaceDeviceUpdateRecovery::ReviewDevices,
                    )
                }
                LedgerUpdateProblem::DeviceUpgradeRequired => {
                    SpaceDeviceUpdateStatus::needs_attention(
                        SpaceDeviceUpdateProblem::DeviceUpgradeRequired,
                        SpaceDeviceUpdateRecovery::UpdateApp,
                    )
                }
                // 组密钥投递给出的需要处理状态原样保留其原因与恢复动作。
                LedgerUpdateProblem::DeviceSecurityUpdateRejected => security_updates,
            },
        };
        let mismatch = if presented.local_status == LedgerMemberStatus::Active {
            if let Some(current) = self
                .local_identity
                .get_current_fingerprint()
                .await
                .map_err(|source| {
                    TrustDependency::LocalIdentity.diagnose(QueryDeviceTrustError::Dependency {
                        source: anyhow::Error::new(source),
                    })
                })?
            {
                let expected = history
                    .admission_facts_for(local_member_instance)
                    .ok_or_else(QueryDeviceTrustError::recovery_required)?;
                let mismatch = current != expected.identity_fingerprint;
                if mismatch {
                    space_device_update = SpaceDeviceUpdateStatus::needs_attention_without_recovery(
                        SpaceDeviceUpdateProblem::LocalIdentityMismatch,
                    );
                }
                Some(mismatch)
            } else {
                None
            }
        } else {
            None
        };
        Ok((
            DeviceTrustStatus {
                revision: view.revision(),
                local_device_id: Some(local_device_id),
                local_membership,
                current_change,
                current_join,
                inbound_pairings,
                pending_inbound_member,
                space_device_update,
                devices,
            },
            mismatch,
        ))
    }

    fn record_identity_change(&self, mismatch: Option<bool>) {
        let Some(mismatch) = mismatch else {
            return;
        };
        let mut previous = self
            .last_identity_mismatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = match (*previous, mismatch) {
            (Some(true), false) => Some(LocalIdentityState::Consistent),
            (Some(true), true) | (Some(false) | None, false) => None,
            (Some(false) | None, true) => Some(LocalIdentityState::Mismatch),
        };
        *previous = Some(mismatch);
        drop(previous);
        if let Some(state) = changed {
            record_local_identity_changed(state);
        }
    }
}

#[cfg(test)]
pub(super) struct MissingLocalIdentity;

#[cfg(test)]
#[async_trait::async_trait]
impl LocalIdentityPort for MissingLocalIdentity {
    async fn create(
        &self,
    ) -> Result<uc_core::security::IdentityFingerprint, uc_core::ports::LocalIdentityError> {
        Err(uc_core::ports::LocalIdentityError::Storage(
            "query is read-only".into(),
        ))
    }

    async fn ensure(
        &self,
    ) -> Result<uc_core::security::IdentityFingerprint, uc_core::ports::LocalIdentityError> {
        Err(uc_core::ports::LocalIdentityError::Storage(
            "query is read-only".into(),
        ))
    }

    async fn get_current_fingerprint(
        &self,
    ) -> Result<Option<uc_core::security::IdentityFingerprint>, uc_core::ports::LocalIdentityError>
    {
        Ok(None)
    }
}

#[cfg(test)]
struct CompletedSecurityUpdates;

#[cfg(test)]
#[async_trait::async_trait]
impl LoadSecurityDeviceUpdateStatusPort for CompletedSecurityUpdates {
    async fn load_security_device_update_status(
        &self,
    ) -> Result<SpaceDeviceUpdateStatus, QueryDeviceTrustError> {
        Ok(SpaceDeviceUpdateStatus::completed())
    }
}

fn pending_change(
    history: &VersionedMembershipHistory,
    local_member: MemberInstanceId,
    devices: &[DeviceTrustDevice],
    ledger: &MembershipLedger,
) -> Result<Option<PendingDeviceTrustChange>, QueryDeviceTrustError> {
    let Some(change_id) = history.pending_removal_decision(local_member) else {
        return Ok(None);
    };
    let event = history
        .event(change_id)
        .ok_or_else(QueryDeviceTrustError::recovery_required)?;
    let target = match &event.operation {
        MembershipOperationV2::RemoveDevice { member } => *member,
        MembershipOperationV2::AddDevice { .. } => {
            return Err(QueryDeviceTrustError::recovery_required());
        }
    };
    let proposed_by_device_id = history
        .admission_facts_for(event.author_member_instance_id)
        .map(|facts| facts.device_id.clone())
        .ok_or_else(QueryDeviceTrustError::recovery_required)?;
    let target_device_id = history
        .admission_facts_for(target)
        .map(|facts| facts.device_id.clone())
        .ok_or_else(QueryDeviceTrustError::recovery_required)?;
    let impact = |apply: bool| -> Result<DeviceTrustImpact, QueryDeviceTrustError> {
        let members = if apply {
            history.effective_members_at(change_id)
        } else {
            history.effective_members()
        };
        let local_removed = !members.contains(&local_member);
        let active_members = history.active_members();
        let mut member_devices = members
            .into_iter()
            .map(|member| {
                history
                    .admission_facts_for(member)
                    .map(|facts| crate::space::membership::MembershipConflictMember {
                        device: uc_core::membership::MembershipConflictDevice {
                            device_id: facts.device_id.clone(),
                            display_name: facts.device_name.clone(),
                        },
                        active: active_members.contains(&member),
                    })
                    .ok_or_else(QueryDeviceTrustError::recovery_required)
            })
            .collect::<Result<Vec<_>, _>>()?;
        member_devices.sort_by(|a, b| a.device.device_id.cmp(&b.device.device_id));
        let member_device_ids: Vec<_> = member_devices
            .iter()
            .map(|member| member.device.device_id.clone())
            .collect();
        let mut usable_device_ids = Vec::new();
        let mut paused_device_ids = Vec::new();
        for device in devices
            .iter()
            .filter(|device| device.membership != DeviceTrustMembership::Removed)
        {
            let usable = !local_removed
                && member_device_ids.contains(&device.device_id)
                && if device.device_id == proposed_by_device_id {
                    apply
                        && matches!(
                            device.sync_state,
                            DeviceTrustSyncState::Usable
                                | DeviceTrustSyncState::Paused(
                                    SpaceMemberPauseReason::PendingLocalDecision
                                )
                        )
                } else {
                    device.sync_state == DeviceTrustSyncState::Usable
                };
            if usable {
                usable_device_ids.push(device.device_id.clone());
            } else {
                paused_device_ids.push(device.device_id.clone());
            }
        }
        let desired_head = if apply {
            Some(change_id)
        } else {
            history.current_head()
        };
        let pending_confirmation_device_ids = usable_device_ids
            .iter()
            .filter(|id| {
                let id = *id;
                ledger.local_device_id() != id
                    && !(apply && id == &proposed_by_device_id)
                    && match ledger.peer(id) {
                        Some(PeerLink::Member(link)) => link
                            .confirmed_position()
                            .and_then(|position| position.event_id),
                        Some(PeerLink::Departing(_)) | None => None,
                    } != desired_head
            })
            .cloned()
            .collect();
        Ok(DeviceTrustImpact {
            members: member_devices,
            member_device_ids,
            usable_device_ids,
            paused_device_ids,
            local_membership: if local_removed {
                DeviceTrustMembership::Removed
            } else {
                devices
                    .iter()
                    .find(|device| device.is_local)
                    .map(|device| device.membership)
                    .ok_or_else(QueryDeviceTrustError::recovery_required)?
            },
            requires_rejoin_device_ids: if apply {
                vec![target_device_id.clone()]
            } else {
                Vec::new()
            },
            pending_confirmation_device_ids,
        })
    };
    let apply_impact = impact(true)?;
    let keep_current_impact = impact(false)?;
    Ok(Some(PendingDeviceTrustChange {
        change_id,
        proposed_by_device_id,
        target_device_ids: vec![target_device_id],
        includes_local_device: target == local_member,
        apply_impact,
        keep_current_impact,
        explanation: uc_core::membership::MembershipConflictExplanation::pending_removal(
            history, change_id,
        )
        .map_err(QueryDeviceTrustError::recovery_required_from)?,
    }))
}

fn membership_of(status: LedgerMemberStatus) -> DeviceTrustMembership {
    match status {
        LedgerMemberStatus::Active => DeviceTrustMembership::Active,
        LedgerMemberStatus::PendingActivation => DeviceTrustMembership::PendingActivation,
        LedgerMemberStatus::Removed => DeviceTrustMembership::Removed,
    }
}

fn relationship_of(relation: PeerRelationView) -> DeviceTrustRelationship {
    match relation {
        PeerRelationView::Local => DeviceTrustRelationship::Local,
        PeerRelationView::Consistent => DeviceTrustRelationship::Consistent,
        PeerRelationView::ConfirmationPending => DeviceTrustRelationship::ConfirmationPending,
        PeerRelationView::PendingLocalDecision => DeviceTrustRelationship::PendingLocalDecision,
        PeerRelationView::AwaitingRemovalAcknowledgement => {
            DeviceTrustRelationship::AwaitingRemovalAcknowledgement
        }
        PeerRelationView::UpgradeRequired => DeviceTrustRelationship::UpgradeRequired,
        PeerRelationView::Diverged => DeviceTrustRelationship::Diverged,
        PeerRelationView::Invalid => DeviceTrustRelationship::Invalid,
        PeerRelationView::Unknown => DeviceTrustRelationship::Unknown,
    }
}

/// 组密钥投递的观察结果；需要处理的投递一律视为被拒绝。
fn security_delivery(status: SpaceDeviceUpdateStatus) -> SecurityDeliveryStatus {
    match status.phase {
        SpaceDeviceUpdatePhase::Completed => SecurityDeliveryStatus::Completed,
        SpaceDeviceUpdatePhase::Updating => SecurityDeliveryStatus::Updating,
        SpaceDeviceUpdatePhase::RetryableFailure => SecurityDeliveryStatus::RetryableFailure {
            next_retry_at_ms: status.next_retry_at_ms.unwrap_or_default(),
        },
        SpaceDeviceUpdatePhase::NeedsAttention => SecurityDeliveryStatus::Rejected,
    }
}
