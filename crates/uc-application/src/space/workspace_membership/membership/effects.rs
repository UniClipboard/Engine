use super::super::*;

impl WorkspaceMembership {
    pub(super) fn enqueue_applied_membership_effects(
        state: &mut SpaceMembershipState,
        events: &[MembershipEvent],
    ) {
        for event in events {
            let event_id = event.event_id();
            if state
                .pending_applied_membership_effects
                .iter()
                .any(|effect| effect.event_id == event_id)
            {
                continue;
            }
            state.pending_applied_membership_effects.push(
                uc_core::membership::PendingAppliedMembershipEffect {
                    event_id,
                    member_facts_completed: !matches!(
                        event.operation,
                        MembershipOperation::AddDevice { .. }
                    ),
                    security_update_completed: event.security_update_payload.is_empty(),
                },
            );
        }
    }

    pub(super) async fn execute_pending_membership_effects(
        &self,
        state: &mut SpaceMembershipState,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        for index in 0..state.pending_applied_membership_effects.len() {
            let effect = state.pending_applied_membership_effects[index].clone();
            let event = state
                .membership_reconciliation
                .as_ref()
                .and_then(|history| history.event(effect.event_id))
                .cloned()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "pending membership effect references an unknown event".to_owned(),
                    )
                })?;

            if !effect.member_facts_completed {
                if let MembershipOperation::AddDevice { admission } = &event.operation {
                    self.save_member_facts(admission, now_ms).await?;
                }
                state.pending_applied_membership_effects[index].member_facts_completed = true;
                self.persist(state).await?;
            }
            if !effect.security_update_completed {
                self.deps
                    .security_updates
                    .apply_group_epoch_update(&event.security_update_payload)
                    .await?;
                state.pending_applied_membership_effects[index].security_update_completed = true;
                self.persist(state).await?;
            }
        }
        state.pending_applied_membership_effects.clear();
        self.persist(state).await?;
        Ok(())
    }

    pub(crate) async fn recover_pending_membership_effects(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let mut state = self.load_state().await?;
        if state.pending_applied_membership_effects.is_empty() {
            return Ok(());
        }
        self.execute_pending_membership_effects(&mut state, self.deps.clock.now_ms())
            .await?;
        self.publish(&state);
        self.notify();
        Ok(())
    }
    pub(crate) async fn deliver_pending_membership_decisions(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.deliver_persisted_v2_removal_decisions().await
    }

    async fn deliver_persisted_v2_removal_decisions(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let Some(encoded) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission::map_repository_error)?
        else {
            return Ok(());
        };
        let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let recipients = history.removal_decision_recipients_for(own);
        if recipients.is_empty() {
            return Ok(());
        }
        let mut candidate_devices = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        candidate_devices.push(self.deps.own_device.clone());
        for recipient_member in recipients {
            let Some(recipient) = history.device_for_member(&recipient_member, &candidate_devices)
            else {
                continue;
            };
            if recipient == self.deps.own_device {
                continue;
            }
            if let Err(error) = self
                .reconcile_membership_history_serialized(
                    &recipient,
                    ReconciliationPeerRole::RestrictedDecisionDelivery,
                )
                .await
            {
                tracing::debug!(
                    recipient = %recipient.as_str(),
                    error = %error,
                    "restricted membership decision delivery deferred"
                );
            }
        }
        Ok(())
    }
}
