use super::super::*;

impl WorkspaceMembership {
    pub(crate) async fn build_current_space_membership_status(
        &self,
    ) -> Result<ActiveSpaceStatusResult, WorkspaceConvergenceError> {
        let state = self.load_state().await?;
        let encoded_history = self
            .deps
            .membership_history_repo
            .load_membership_history()
            .await
            .map_err(WorkspaceConvergenceError::from)?
            .ok_or(WorkspaceConvergenceError::Unavailable)?;
        let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let local_membership = if history.active_members().contains(&own_instance) {
            DeviceMembership::Active
        } else {
            DeviceMembership::Removed
        };
        let roster = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;

        Ok(build_active_space_status(ActiveSpaceStatusFacts {
            state,
            history,
            own_instance,
            local_membership,
            roster,
            presence: Arc::clone(&self.deps.presence),
            local_device_id: self.deps.own_device.clone(),
        })
        .await)
    }

    pub async fn decide_membership_removal(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        self.decide_membership_removal_locked(removal_event_id, decision)
            .await
    }

    async fn decide_membership_removal_v2(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<Option<WorkspaceSnapshot>, WorkspaceConvergenceError> {
        let Some(encoded_history) = self
            .deps
            .membership_history_repo
            .load_membership_history()
            .await
            .map_err(WorkspaceConvergenceError::from)?
        else {
            return Ok(None);
        };
        let mut history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let Some(removal) = history.event(removal_event_id).cloned() else {
            return Ok(None);
        };
        if !matches!(
            removal.operation,
            MembershipOperationV2::RemoveDevice { .. }
        ) {
            return Ok(None);
        }
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own = own_credential.member_instance_id(&self.deps.own_device);
        if let Some(completed) = history.decision_for(removal_event_id, own) {
            if completed.decision != decision {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "membership removal was completed with a different decision".to_owned(),
                ));
            }
            return self.query().await.map(Some);
        }
        if history.pending_removal_decision(own) != Some(removal_event_id) {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "membership removal is no longer pending".to_owned(),
            ));
        }
        let mut signed_decision = history
            .create_unsigned_local_removal_decision(
                removal_event_id,
                own,
                &own_credential,
                decision,
                uuid::Uuid::new_v4().into_bytes(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        signed_decision.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&signed_decision.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        history
            .apply_signed_local_removal_decision(
                signed_decision,
                own,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let replacement = history
            .encode_persisted_v2()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;

        let _guard = self.state_write_lock.lock().await;
        let mut state = self.load_state().await?;
        self.deps
            .membership_history_repo
            .compare_and_replace_membership_history(Some(&encoded_history), &replacement)
            .await
            .map_err(WorkspaceConvergenceError::from)?;
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
        let decision_author =
            history.device_for_member(&removal.author_member_instance_id, &candidate_devices);
        if let Some(author) = decision_author.as_ref() {
            self.update_peer_history_relationship(
                &mut state,
                author.clone(),
                if decision == RemovalDecision::Accept {
                    MembershipHistoryRelationship::Consistent
                } else {
                    MembershipHistoryRelationship::Diverged
                },
                self.deps.clock.now_ms(),
            )?;
            self.persist(&state).await?;
        }
        self.publish(&state);
        drop(_guard);
        self.deliver_pending_membership_decisions().await?;
        self.query().await.map(Some)
    }

    async fn decide_membership_removal_locked(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        if let Some(snapshot) = self
            .decide_membership_removal_v2(removal_event_id, decision)
            .await?
        {
            return Ok(snapshot);
        }
        let (snapshot, recipients, signed_decision) = {
            let _guard = self.state_write_lock.lock().await;
            let now_ms = self.deps.clock.now_ms();
            let mut state = self.load_state().await?;
            let own_member_instance_id = state
                .own_instance
                .ok_or(WorkspaceConvergenceError::NotAMember)?;
            let history = state
                .membership_reconciliation
                .as_ref()
                .ok_or(WorkspaceConvergenceError::NotAMember)?;
            if let Some(completed) = history.local_removal_decision(removal_event_id) {
                return if completed == decision {
                    Ok(state.snapshot())
                } else {
                    Err(WorkspaceConvergenceError::Inconsistent(
                        "membership removal was completed with a different decision".to_owned(),
                    ))
                };
            }
            if history.pending_removal_decision() != Some(removal_event_id) {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "membership removal is no longer pending".to_owned(),
                ));
            }
            let removal =
                history
                    .event(removal_event_id)
                    .ok_or(WorkspaceConvergenceError::Inconsistent(
                        "membership removal is unknown".to_owned(),
                    ))?;
            let removal_author_device_id = history
                .device_for_member_before(removal_event_id, &removal.author_member_instance_id)
                .ok_or(WorkspaceConvergenceError::Inconsistent(
                    "membership removal author is unknown".to_owned(),
                ))?;
            let resulting_members_digest = match decision {
                RemovalDecision::Accept => removal.resulting_members_digest,
                RemovalDecision::Reject => history.applied_members_digest().ok_or(
                    WorkspaceConvergenceError::Inconsistent(
                        "membership removal has no applied parent".to_owned(),
                    ),
                )?,
            };
            let mut recipients = history
                .effective_members()
                .into_iter()
                .filter_map(|member| history.device_for_member(&member))
                .filter(|device| *device != self.deps.own_device)
                .collect::<Vec<_>>();
            recipients.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            recipients.dedup();
            let unsigned = MembershipDecision::new(
                state.space_lineage.clone(),
                removal_event_id,
                own_member_instance_id,
                decision,
                history.applied_head(),
                resulting_members_digest,
                uuid::Uuid::new_v4().into_bytes(),
                Vec::new(),
            );
            let signature = self
                .deps
                .member_signatures
                .sign_current_member_payload(&unsigned.signing_payload())
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            let signed_decision = MembershipDecision::new(
                unsigned.lineage_id,
                unsigned.removal_event_id,
                unsigned.decided_by_member_instance_id,
                unsigned.decision,
                unsigned.observed_applied_head,
                unsigned.resulting_members_digest,
                unsigned.decision_nonce,
                signature,
            );
            let applied_events = {
                let history = state
                    .membership_reconciliation
                    .as_mut()
                    .ok_or(WorkspaceConvergenceError::NotAMember)?;
                let previous_applied_head = history.applied_head();
                history
                    .record_decision(signed_decision.clone())
                    .map_err(|_| {
                        WorkspaceConvergenceError::Inconsistent(
                            "membership removal decision was rejected".to_owned(),
                        )
                    })?;
                history.newly_applied_events_after(previous_applied_head)
            };
            for recipient in &recipients {
                state.pending_membership_decision_deliveries.push(
                    uc_core::membership::PendingMembershipDecisionDelivery {
                        recipient: recipient.clone(),
                        decision: signed_decision.clone(),
                    },
                );
            }
            if decision == RemovalDecision::Accept {
                Self::enqueue_applied_membership_effects(&mut state, &applied_events);
                self.persist(&state).await?;
                self.execute_pending_membership_effects(&mut state, now_ms)
                    .await?;
            }
            let relationship = match decision {
                RemovalDecision::Accept => MembershipHistoryRelationship::Consistent,
                RemovalDecision::Reject => MembershipHistoryRelationship::Diverged,
            };
            self.update_peer_history_relationship(
                &mut state,
                removal_author_device_id.clone(),
                relationship,
                now_ms,
            )?;
            self.persist(&state).await?;
            self.publish(&state);
            (state.snapshot(), recipients, signed_decision)
        };

        let _ = recipients;
        let _ = signed_decision;
        self.deliver_pending_membership_decisions().await?;
        Ok(snapshot)
    }

    pub(super) fn update_peer_history_relationship(
        &self,
        state: &mut SpaceMembershipState,
        peer: DeviceId,
        relationship: MembershipHistoryRelationship,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        state
            .apply(
                WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated { peer, relationship },
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("history relationship rejected".to_owned())
            })?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn submit_legacy_removal_for_test(
        &self,
        target: &DeviceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_write_lock.lock().await;
        let mut state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let own = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let target_member = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .effective_members()
            .into_iter()
            .find(|member| {
                state
                    .membership_reconciliation
                    .as_ref()
                    .and_then(|history| history.device_for_member(member))
                    .as_ref()
                    == Some(target)
            })
            .ok_or(WorkspaceConvergenceError::UnknownTarget)?;
        if target_member == own {
            return Err(WorkspaceConvergenceError::SelfTarget);
        }
        let security_state_digest = state
            .current_digest()
            .map(|digest| *digest.as_bytes())
            .unwrap_or([0; 32]);
        self.record_local_removal_history(&mut state, target_member, security_state_digest)
            .await?;
        self.persist(&state).await?;
        self.publish(&state);
        Ok(state.snapshot())
    }
}
