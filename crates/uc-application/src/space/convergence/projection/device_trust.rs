use super::super::*;

impl WorkspaceConvergence {
    pub async fn query_device_trust(
        &self,
    ) -> Result<DeviceTrustSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let state = self.load_state().await?;
        let local_device_id = self.deps.own_device.clone();
        let workspace_unverifiable = state.failure_category.is_some();
        let roster = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let mut candidate_devices = roster
            .iter()
            .map(|member| member.device_id.clone())
            .collect::<Vec<_>>();
        candidate_devices.push(local_device_id.clone());
        candidate_devices.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidate_devices.dedup();
        let v2_history = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission::map_repository_error)?
            .map(|encoded| {
                uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                    &encoded,
                    self.deps.historical_membership_signatures.as_ref(),
                )
            })
            .transpose()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let v2_own_instance = if v2_history.is_some() {
            Some(
                self.deps
                    .member_signatures
                    .current_member_instance(&local_device_id)
                    .await
                    .map_err(|_| WorkspaceConvergenceError::Unavailable)?,
            )
        } else {
            None
        };
        let local_membership =
            if let (Some(history), Some(own_instance)) = (v2_history.as_ref(), v2_own_instance) {
                if history.active_members().contains(&own_instance) {
                    DeviceMembership::Active
                } else {
                    DeviceMembership::Removed
                }
            } else if state.own_instance.is_none() {
                match uc_core::membership::CurrentWorkspacePeerScopePort::snapshot(self).await {
                    Ok(scope) => match scope.local_membership {
                        uc_core::membership::CurrentWorkspaceLocalMembership::Active => {
                            DeviceMembership::Active
                        }
                        uc_core::membership::CurrentWorkspaceLocalMembership::Removed => {
                            DeviceMembership::Removed
                        }
                    },
                    Err(_) => DeviceMembership::Unavailable,
                }
            } else if state.removed {
                DeviceMembership::Removed
            } else {
                DeviceMembership::Active
            };
        let history = state.membership_reconciliation.as_ref();
        let pending_facts = if workspace_unverifiable {
            None
        } else if let (Some(history), Some(own_instance)) = (v2_history.as_ref(), v2_own_instance) {
            history
                .pending_removal_decision(own_instance)
                .and_then(|removal_event_id| {
                    let event = history.event(removal_event_id)?;
                    let MembershipOperationV2::RemoveDevice { member } = event.operation else {
                        return None;
                    };
                    let proposed_by_device_id = history
                        .device_for_member(&event.author_member_instance_id, &candidate_devices)?;
                    let target_device_id =
                        history.device_for_member(&member, &candidate_devices)?;
                    Some(uc_core::membership::PendingRemovalFacts::new(
                        removal_event_id,
                        proposed_by_device_id,
                        vec![target_device_id],
                        [member].into(),
                    ))
                })
        } else {
            history.and_then(|history| history.pending_removal_facts())
        };
        let includes_local_device = pending_facts.as_ref().is_some_and(|facts| {
            v2_own_instance
                .or(state.own_instance)
                .is_some_and(|member| facts.includes_member(member))
        });

        let mut names = BTreeMap::new();
        if let Some(history) = history {
            for admission in history.admitted_device_facts() {
                names.insert(admission.device_id, admission.device_name);
            }
        }
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
                self.deps.presence.current_state(&device_id).await
            };
            let membership = if is_local {
                local_membership
            } else if v2_history.as_ref().is_some_and(|history| {
                history.effective_members().iter().any(|member| {
                    history
                        .device_for_member(member, &candidate_devices)
                        .as_ref()
                        == Some(&device_id)
                })
            }) || history.is_some_and(|history| history.is_device_effective(&device_id))
            {
                DeviceMembership::Active
            } else if v2_history
                .as_ref()
                .is_some_and(|history| history.has_admitted_device(&device_id, &candidate_devices))
                || history.is_some_and(|history| history.has_admitted_device(&device_id))
            {
                DeviceMembership::Removed
            } else {
                DeviceMembership::Unknown
            };
            let relationship = state.peer_history_relationships.get(&device_id);
            let group_relationship = if workspace_unverifiable {
                GroupRelationship::Unverifiable
            } else {
                match relationship {
                    Some(MembershipHistoryRelationship::Consistent) => {
                        GroupRelationship::Consistent
                    }
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
                    vec![DeviceTrustAction::RejoinDeviceGroup],
                    Some(ActionUnavailableReason::LocalDeviceRemoved),
                ),
                SyncRelationship::PausedUpgradeRequired if is_local => {
                    (vec![DeviceTrustAction::UpdateThisDevice], None)
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
            devices.push(DeviceTrustRelationship {
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
            DeviceTrustChange {
                change_id: facts.removal_event_id,
                proposed_by_device_id: facts.proposed_by_device_id,
                target_device_ids: facts.target_device_ids.clone(),
                includes_local_device,
                apply_impact: DeviceTrustImpact {
                    usable_device_ids: apply_usable,
                    paused_device_ids: Vec::new(),
                    local_device_outcome: if includes_local_device {
                        DeviceMembership::Removed
                    } else {
                        local_membership
                    },
                    requires_rejoin_device_ids: facts.target_device_ids,
                },
                keep_current_impact: DeviceTrustImpact {
                    usable_device_ids: keep_usable,
                    paused_device_ids: paused,
                    local_device_outcome: local_membership,
                    requires_rejoin_device_ids: Vec::new(),
                },
                allowed_choices: vec![
                    DeviceTrustChoice::ApplyChange,
                    DeviceTrustChoice::KeepCurrentDeviceGroup,
                ],
                blocked_reason: includes_local_device
                    .then_some(ActionUnavailableReason::LocalDeviceConfirmationRequired),
            }
        });
        let current_join = self.admission.current_local_join().await?;
        let pending_inbound_member = self
            .admission
            .pending_inbound_member(&state.space_lineage)
            .await?;
        let (allowed_actions, blocked_reason) = if workspace_unverifiable {
            (
                Vec::new(),
                Some(ActionUnavailableReason::DeviceFactsUnverifiable),
            )
        } else {
            match &current_change {
                Some(change) if change.includes_local_device => (
                    vec![
                        DeviceTrustAction::KeepCurrentDeviceGroup,
                        DeviceTrustAction::ConfirmApplyRemovesLocalDevice,
                    ],
                    Some(ActionUnavailableReason::LocalDeviceConfirmationRequired),
                ),
                Some(_) => (
                    vec![
                        DeviceTrustAction::ApplyCurrentChange,
                        DeviceTrustAction::KeepCurrentDeviceGroup,
                    ],
                    None,
                ),
                None if local_membership == DeviceMembership::Removed => (
                    vec![DeviceTrustAction::RejoinDeviceGroup],
                    Some(ActionUnavailableReason::LocalDeviceRemoved),
                ),
                None => (Vec::new(), Some(ActionUnavailableReason::NoCurrentChange)),
            }
        };
        let admission_revision = self
            .deps
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission::map_repository_error)?
            .device_trust_revision;
        Ok(DeviceTrustSnapshot {
            revision: state.revision.max(admission_revision),
            local_device_id,
            local_membership,
            current_change,
            current_join,
            pending_inbound_member,
            devices,
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions,
            blocked_reason,
            updated_at_ms: state.updated_at_ms,
        })
    }

    /// Load the current workspace snapshot without changing any state.
    pub async fn query(&self) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let state = self.load_state().await?;
        let mut snapshot = state.snapshot();
        let Some(scope) = self
            .v2_current_peer_snapshot(&state)
            .await
            .map_err(|error| {
                WorkspaceConvergenceError::Inconsistent(format!(
                    "current V2 member scope is unavailable: {error:?}"
                ))
            })?
        else {
            return Ok(snapshot);
        };
        let encoded_history = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission::map_repository_error)?
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "current V2 member history disappeared during query".to_owned(),
                )
            })?;
        let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| {
            WorkspaceConvergenceError::Inconsistent(format!(
                "current V2 member history is invalid: {error}"
            ))
        })?;
        let position = history.current_position().map_err(|error| {
            WorkspaceConvergenceError::Inconsistent(format!(
                "current V2 member history position is invalid: {error}"
            ))
        })?;
        let metadata = self
            .deps
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission::map_repository_error)?;
        snapshot.revision = snapshot.revision.max(metadata.device_trust_revision);
        snapshot.history_event_count =
            usize::try_from(position.depth.saturating_add(1)).unwrap_or(usize::MAX);
        snapshot.effective_member_count = history.active_members().len();
        snapshot.convergence_digest = Some(uc_core::membership::WorkspaceDigest::from_bytes(
            position.history_digest,
        ));
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        snapshot.pending_removal_decision_event_id = history.pending_removal_decision(own_instance);
        snapshot.removed =
            scope.local_membership == uc_core::membership::CurrentWorkspaceLocalMembership::Removed;
        Ok(snapshot)
    }
}
