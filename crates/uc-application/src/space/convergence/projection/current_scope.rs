use super::super::*;

impl WorkspaceConvergence {
    pub(in crate::space::convergence) async fn v2_current_peer_snapshot(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<
        Option<uc_core::membership::CurrentWorkspacePeerSnapshot>,
        uc_core::membership::CurrentWorkspacePeerScopeError,
    > {
        use uc_core::membership::{
            AdmissionAttemptRepositoryError, AdmissionTerminalResultV1,
            CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopeError,
            CurrentWorkspacePeerScopeSource, CurrentWorkspacePeerSnapshot,
            MembershipHistoryV2Error, VersionedMembershipHistory,
        };

        let map_repository_error = |error| match error {
            AdmissionAttemptRepositoryError::Locked => CurrentWorkspacePeerScopeError::Locked,
            AdmissionAttemptRepositoryError::Corrupt => CurrentWorkspacePeerScopeError::Corrupt,
            _ => CurrentWorkspacePeerScopeError::Unavailable,
        };
        let Some(encoded_history) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(|error| {
                let (category, mapped) = match error {
                    AdmissionAttemptRepositoryError::Locked => {
                        ("locked", CurrentWorkspacePeerScopeError::Locked)
                    }
                    AdmissionAttemptRepositoryError::Corrupt => {
                        ("corrupt", CurrentWorkspacePeerScopeError::Corrupt)
                    }
                    _ => ("unavailable", CurrentWorkspacePeerScopeError::Unavailable),
                };
                tracing::warn!(
                    error_kind = "current_peer_scope_history_read",
                    error_category = category,
                    "[DEBUG-cps1] current peer scope diagnostic"
                );
                mapped
            })?
        else {
            tracing::debug!(
                error_kind = "current_peer_scope_v2_history_absent",
                "[DEBUG-cps1] current peer scope diagnostic"
            );
            return Ok(None);
        };
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| match error {
            MembershipHistoryV2Error::UpgradeRequired => {
                tracing::warn!(
                    error_kind = "current_peer_scope_history_decode",
                    error_category = "upgrade_required",
                    "[DEBUG-cps1] current peer scope diagnostic"
                );
                CurrentWorkspacePeerScopeError::Unavailable
            }
            _ => {
                tracing::warn!(
                    error_kind = "current_peer_scope_history_decode",
                    error_category = "corrupt",
                    "[DEBUG-cps1] current peer scope diagnostic"
                );
                CurrentWorkspacePeerScopeError::Corrupt
            }
        })?;
        let local_join = self
            .deps
            .admission_attempts
            .project_current_local_join()
            .await
            .map_err(|error| {
                let (category, mapped) = match error {
                    AdmissionAttemptRepositoryError::Locked => {
                        ("locked", CurrentWorkspacePeerScopeError::Locked)
                    }
                    AdmissionAttemptRepositoryError::Corrupt => {
                        ("corrupt", CurrentWorkspacePeerScopeError::Corrupt)
                    }
                    _ => ("unavailable", CurrentWorkspacePeerScopeError::Unavailable),
                };
                tracing::warn!(
                    error_kind = "current_peer_scope_local_join_read",
                    error_category = category,
                    "[DEBUG-cps1] current peer scope diagnostic"
                );
                mapped
            })?;
        if history.lineage_id() != state.space_lineage {
            if let Some(join) = &local_join {
                if join.terminal_result.is_none() {
                    let attempt = self
                        .deps
                        .admission_attempts
                        .load(join.attempt_id)
                        .await
                        .map_err(map_repository_error)?
                        .ok_or(CurrentWorkspacePeerScopeError::Corrupt)?;
                    let transition = attempt
                        .space_transition
                        .as_deref()
                        .and_then(uc_core::membership::AdmissionSpaceTransitionV2::decode);
                    if transition.as_ref().is_some_and(|transition| {
                        matches!(
                            transition,
                            uc_core::membership::AdmissionSpaceTransitionV2::CrossSpace(item)
                                if transition.attempt_id() == join.attempt_id
                                    && item.source_space_id == state.space_lineage
                                    && item.target_space_id == history.lineage_id()
                                    && transition.phase_rank()
                                        < transition.activation_started_rank()
                        )
                    }) {
                        return Ok(None);
                    }
                } else if join.terminal_result == Some(AdmissionTerminalResultV1::Rejected) {
                    let terminal = self
                        .deps
                        .admission_attempts
                        .load_terminal(join.attempt_id)
                        .await
                        .map_err(map_repository_error)?
                        .ok_or(CurrentWorkspacePeerScopeError::Corrupt)?;
                    if terminal
                        .candidate_event_id
                        .is_some_and(|event_id| history.contains_event_id(&event_id))
                    {
                        return Ok(None);
                    }
                }
            }
            tracing::warn!(
                error_kind = "current_peer_scope_lineage_mismatch",
                local_join_present = local_join.is_some(),
                "[DEBUG-cps1] current peer scope diagnostic"
            );
            return Err(CurrentWorkspacePeerScopeError::Corrupt);
        }

        let members = self.deps.member_repo.list().await.map_err(|_| {
            tracing::warn!(
                error_kind = "current_peer_scope_member_roster_read",
                error_category = "unavailable",
                "[DEBUG-cps1] current peer scope diagnostic"
            );
            CurrentWorkspacePeerScopeError::Unavailable
        })?;
        let mut candidate_devices = members
            .into_iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        candidate_devices.push(self.deps.own_device.clone());
        candidate_devices.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidate_devices.dedup();

        let active_member_count = history.active_members().len();
        let active_devices = history
            .active_members()
            .iter()
            .filter_map(|member| history.device_for_member(member, &candidate_devices))
            .collect::<Vec<_>>();
        if active_devices.len() != active_member_count {
            tracing::warn!(
                error_kind = "current_peer_scope_active_member_mapping_missing",
                active_member_count,
                candidate_device_count = candidate_devices.len(),
                unresolved_member_count = active_member_count - active_devices.len(),
                "[DEBUG-cps1] current peer scope diagnostic"
            );
            return Err(CurrentWorkspacePeerScopeError::Unavailable);
        }
        let local_join_does_not_block_current_history =
            local_join.is_none_or(|join| join.terminal_result.is_some());
        let local_membership = if active_devices.contains(&self.deps.own_device)
            && local_join_does_not_block_current_history
        {
            CurrentWorkspaceLocalMembership::Active
        } else {
            CurrentWorkspaceLocalMembership::Removed
        };
        let mut peer_device_ids = if local_membership == CurrentWorkspaceLocalMembership::Active {
            active_devices
                .into_iter()
                .filter(|device| *device != self.deps.own_device)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        peer_device_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        peer_device_ids.dedup();

        Ok(Some(CurrentWorkspacePeerSnapshot {
            revision: state.revision,
            source: CurrentWorkspacePeerScopeSource::CurrentHistory,
            local_membership,
            peer_device_ids,
        }))
    }
}

#[async_trait]
impl uc_core::membership::CurrentWorkspacePeerScopePort for WorkspaceConvergence {
    async fn snapshot(
        &self,
    ) -> Result<
        uc_core::membership::CurrentWorkspacePeerSnapshot,
        uc_core::membership::CurrentWorkspacePeerScopeError,
    > {
        use uc_core::membership::{
            CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopeError,
            CurrentWorkspacePeerScopeSource, CurrentWorkspacePeerSnapshot,
        };

        let state = self.load_state().await.map_err(|error| match error {
            WorkspaceConvergenceError::Repository(
                uc_core::membership::WorkspaceConvergenceRepositoryError::Locked,
            ) => CurrentWorkspacePeerScopeError::Locked,
            WorkspaceConvergenceError::Repository(
                uc_core::membership::WorkspaceConvergenceRepositoryError::Corrupt,
            ) => CurrentWorkspacePeerScopeError::Corrupt,
            _ => CurrentWorkspacePeerScopeError::Unavailable,
        })?;
        if let Some(snapshot) = self.v2_current_peer_snapshot(&state).await? {
            return Ok(snapshot);
        }
        let history = state
            .membership_reconciliation
            .as_ref()
            .filter(|history| history.applied_head().is_some());
        let Some(history) = history else {
            let members = self.deps.member_repo.list().await.map_err(|_| {
                tracing::warn!(
                    error_kind = "current_peer_scope_legacy_member_roster_read",
                    error_category = "unavailable",
                    "[DEBUG-cps1] current peer scope diagnostic"
                );
                CurrentWorkspacePeerScopeError::Unavailable
            })?;
            let member_ids = members
                .iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>();
            let mut protection_member_ids = member_ids.clone();
            protection_member_ids.push(self.deps.own_device);
            protection_member_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            protection_member_ids.dedup();
            let protection = self
                .deps
                .space_protection
                .query_space_protection(&protection_member_ids)
                .await
                .map_err(|error| match error {
                    uc_core::membership::SpaceProtectionError::Corrupted => {
                        tracing::warn!(
                            error_kind = "current_peer_scope_legacy_protection_read",
                            error_category = "corrupt",
                            "[DEBUG-cps1] current peer scope diagnostic"
                        );
                        CurrentWorkspacePeerScopeError::Corrupt
                    }
                    _ => {
                        tracing::warn!(
                            error_kind = "current_peer_scope_legacy_protection_read",
                            error_category = "unavailable",
                            "[DEBUG-cps1] current peer scope diagnostic"
                        );
                        CurrentWorkspacePeerScopeError::Unavailable
                    }
                })?;
            if protection.mode != uc_core::membership::SpaceProtectionMode::Legacy
                && !state.migrated_from_pre_adr_020
            {
                tracing::warn!(
                    error_kind = "current_peer_scope_current_history_absent",
                    protection_mode = ?protection.mode,
                    migrated_from_pre_adr_020 = state.migrated_from_pre_adr_020,
                    member_record_count = member_ids.len(),
                    "[DEBUG-cps1] current peer scope diagnostic"
                );
                return Err(CurrentWorkspacePeerScopeError::Unavailable);
            }
            let local_is_member = protection.mode
                == uc_core::membership::SpaceProtectionMode::Legacy
                || protection.members.iter().any(|member| {
                    member.device_id == self.deps.own_device
                        && member.status == uc_core::membership::MemberProtectionStatus::Protected
                });
            let mut peer_device_ids = if local_is_member {
                member_ids
                    .into_iter()
                    .filter(|device| *device != self.deps.own_device)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            peer_device_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            peer_device_ids.dedup();
            return Ok(CurrentWorkspacePeerSnapshot {
                revision: state.revision,
                source: CurrentWorkspacePeerScopeSource::Legacy,
                local_membership: if local_is_member {
                    CurrentWorkspaceLocalMembership::Active
                } else {
                    CurrentWorkspaceLocalMembership::Removed
                },
                peer_device_ids,
            });
        };
        let local_membership = if state.removed
            || state
                .own_instance
                .is_none_or(|instance| !history.effective_members().contains(&instance))
        {
            CurrentWorkspaceLocalMembership::Removed
        } else {
            CurrentWorkspaceLocalMembership::Active
        };
        let pending_additions = state
            .pending_applied_membership_effects
            .iter()
            .filter_map(|effect| history.event(effect.event_id))
            .filter_map(|event| match &event.operation {
                MembershipOperation::AddDevice { admission } => Some(admission.device_id.clone()),
                MembershipOperation::RemoveDevice { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut peer_device_ids = if local_membership == CurrentWorkspaceLocalMembership::Active {
            history
                .effective_members()
                .into_iter()
                .filter_map(|member| history.device_for_member(&member))
                .filter(|device| *device != self.deps.own_device)
                .filter(|device| !pending_additions.contains(device))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        peer_device_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        peer_device_ids.dedup();

        Ok(CurrentWorkspacePeerSnapshot {
            revision: state.revision,
            source: CurrentWorkspacePeerScopeSource::CurrentHistory,
            local_membership,
            peer_device_ids,
        })
    }
}

impl WorkspaceConvergence {
    /// Whether the local device may currently drive content sends.
    pub async fn locally_removed(&self, device_id: &DeviceId) -> bool {
        let scope = match uc_core::membership::CurrentWorkspacePeerScopePort::snapshot(self).await {
            Ok(scope) => scope,
            Err(error) => {
                tracing::warn!(
                    peer = %device_id.as_str(),
                    error = ?error,
                    "content exchange denied because current member scope is unavailable"
                );
                return true;
            }
        };
        if !scope.peer_device_ids.contains(device_id) {
            tracing::warn!(
                peer = %device_id.as_str(),
                source = ?scope.source,
                "content exchange denied because peer is outside current member scope"
            );
            return true;
        }
        match self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
        {
            Ok(Some(_)) => {
                return self
                    .load_state()
                    .await
                    .map_or(true, |state| !state.allows_normal_exchange(device_id));
            }
            Ok(None) => {}
            Err(_) => return true,
        }
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => return true,
        };
        state.removed
            || state.is_device_removed(device_id)
            || !state.allows_normal_exchange(device_id)
    }

    /// Whether the local member instance has observed its own removal.
    pub async fn own_instance_removed(&self) -> bool {
        self.load_state().await.map_or(true, |state| state.removed)
    }
}

#[async_trait]
impl uc_core::membership::ContentExchangeGatePort for WorkspaceConvergence {
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool {
        self.locally_removed(device_id).await
    }
}
