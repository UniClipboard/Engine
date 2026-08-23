use super::super::*;

impl WorkspaceMembership {
    /// Receive one bounded membership-history message from an already
    /// authenticated member connection. This owner persists the verified
    /// result before returning an acknowledgement, so callers never compose
    /// history application with a separate persistence step.
    pub async fn handle_membership_history(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let response = match message {
            MembershipHistoryMessage::HistoryPageV2(page) => {
                self.receive_membership_history_v2(&mut state, source_device_id, page, now_ms)
                    .await?
            }
            MembershipHistoryMessage::AckV2(ack) => MembershipHistoryMessage::AckV2(ack),
        };
        Ok(response)
    }

    /// Start one bounded reconciliation exchange after a peer becomes
    /// reachable. The caller supplies only the authenticated peer identity;
    /// this owner builds every protocol message and persists every reply.
    pub async fn reconcile_membership_history_with_peer(
        &self,
        peer: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.reconcile_membership_history_serialized(peer, ReconciliationPeerRole::RuntimePeer)
            .await
    }

    pub(super) async fn reconcile_membership_history_serialized(
        &self,
        peer: &DeviceId,
        peer_role: ReconciliationPeerRole,
    ) -> Result<(), WorkspaceConvergenceError> {
        let peer_lock = {
            let mut locks = self.peer_reconciliation_locks.lock().await;
            Arc::clone(
                locks
                    .entry(peer.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _peer_guard = peer_lock.lock().await;
        self.reconcile_membership_history(peer, peer_role).await
    }

    async fn record_current_peer_confirmation(
        &self,
        peer: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if matches!(
            state.peer_history_relationships.get(peer),
            None | Some(MembershipHistoryRelationship::Unknown)
                | Some(MembershipHistoryRelationship::UpgradeRequired)
        ) {
            self.update_peer_history_relationship(
                &mut state,
                peer.clone(),
                MembershipHistoryRelationship::Consistent,
                now_ms,
            )?;
            self.persist(&state).await?;
            self.publish(&state);
            self.notify();
        }
        Ok(())
    }

    pub(super) async fn reconcile_membership_history_with_sponsor(
        &self,
        sponsor: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.reconcile_membership_history(sponsor, ReconciliationPeerRole::AuthenticatedSponsor)
            .await
    }

    async fn reconcile_membership_history(
        &self,
        peer: &DeviceId,
        peer_role: ReconciliationPeerRole,
    ) -> Result<(), WorkspaceConvergenceError> {
        let pages = {
            let _guard = self.state_lock.lock().await;
            let state = self.load_state().await?;
            let restricted_decision_delivery = matches!(
                peer_role,
                ReconciliationPeerRole::RestrictedDecisionDelivery
            );
            if !restricted_decision_delivery
                && (state.removed
                    || matches!(
                        state.peer_history_relationships.get(peer),
                        Some(
                            MembershipHistoryRelationship::PendingRemovalDecision
                                | MembershipHistoryRelationship::Diverged
                                | MembershipHistoryRelationship::Invalid
                        )
                    ))
            {
                return Ok(());
            }
            let Some(encoded) = self
                .deps
                .admission_attempts
                .load_membership_history_v2()
                .await
                .map_err(admission::map_repository_error)?
            else {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            };
            let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            if history.lineage_id() != state.space_lineage {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            let own_admission = self
                .local_admission_facts(Some(
                    self.deps
                        .member_signatures
                        .current_member_instance(&self.deps.own_device)
                        .await
                        .map_err(|_| WorkspaceConvergenceError::Unavailable)?,
                ))
                .await?;
            history
                .export_reconciliation_pages_v2(own_admission)
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        };
        let transfer_id = pages
            .first()
            .map(|page| page.transfer_id())
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "membership history page export was empty".to_owned(),
                )
            })?;
        let mut next_page_index = 0u32;
        for _ in 0..=pages.len() {
            let page = pages
                .get(next_page_index as usize)
                .cloned()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "membership history requested an invalid page".to_owned(),
                    )
                })?;
            let reply = self
                .deps
                .membership_history_exchange
                .exchange_membership_history(peer, MembershipHistoryMessage::HistoryPageV2(page))
                .await
                .map_err(|error| match error {
                    MembershipHistoryExchangeError::Offline
                    | MembershipHistoryExchangeError::Transport => {
                        WorkspaceConvergenceError::Unavailable
                    }
                    MembershipHistoryExchangeError::Rejected => {
                        WorkspaceConvergenceError::Inconsistent(
                            "membership history exchange rejected".to_owned(),
                        )
                    }
                })?;
            let MembershipHistoryMessage::AckV2(ack) = reply else {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "membership history exchange returned an invalid response".to_owned(),
                ));
            };
            match ack {
                uc_core::membership::MembershipHistoryV2Ack::Continue {
                    transfer_id: acknowledged_transfer,
                    next_page_index: requested_page,
                } if acknowledged_transfer == transfer_id
                    && (requested_page as usize) < pages.len() =>
                {
                    next_page_index = requested_page;
                }
                uc_core::membership::MembershipHistoryV2Ack::Consistent
                | uc_core::membership::MembershipHistoryV2Ack::UpdatesApplied
                    if next_page_index as usize + 1 == pages.len() =>
                {
                    self.record_current_peer_confirmation(peer).await?;
                    return Ok(());
                }
                uc_core::membership::MembershipHistoryV2Ack::Diverged
                | uc_core::membership::MembershipHistoryV2Ack::Invalid => return Ok(()),
                _ => {
                    return Err(WorkspaceConvergenceError::Inconsistent(
                        "membership history acknowledgement is inconsistent".to_owned(),
                    ))
                }
            }
        }
        Err(WorkspaceConvergenceError::Inconsistent(
            "membership history paging did not advance".to_owned(),
        ))
    }

    async fn receive_membership_history_v2(
        &self,
        state: &mut SpaceMembershipState,
        source_device_id: &DeviceId,
        page: uc_core::membership::MembershipHistoryPageV2,
        now_ms: i64,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        use uc_core::membership::{
            MembershipHistoryV2Ack, PendingMembershipHistoryTransferV2, VersionedMembershipHistory,
        };

        let transfer_id = page.transfer_id();
        let page_count = page.page_count();
        let page_index = page.page_index();
        if page.validate_envelope().is_err() {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let pending = state
            .pending_membership_history_transfers
            .entry(source_device_id.clone())
            .or_insert_with(|| PendingMembershipHistoryTransferV2 {
                transfer_id,
                page_count,
                pages: Vec::new(),
            });
        if pending.transfer_id != transfer_id || pending.page_count != page_count {
            state
                .pending_membership_history_transfers
                .remove(source_device_id);
            self.persist(state).await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let expected_index = u32::try_from(pending.pages.len()).map_err(|_| {
            WorkspaceConvergenceError::Inconsistent(
                "membership history page count exceeds the protocol range".to_owned(),
            )
        })?;
        if page_index < expected_index {
            if pending.pages.get(page_index as usize) != Some(&page) {
                state
                    .pending_membership_history_transfers
                    .remove(source_device_id);
                self.persist(state).await?;
                return Ok(MembershipHistoryMessage::AckV2(
                    MembershipHistoryV2Ack::Invalid,
                ));
            }
        } else if page_index == expected_index {
            pending.pages.push(page);
        } else {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Continue {
                    transfer_id,
                    next_page_index: expected_index,
                },
            ));
        }
        self.persist(state).await?;
        let received_count = state
            .pending_membership_history_transfers
            .get(source_device_id)
            .map(|transfer| transfer.pages.len())
            .unwrap_or_default();
        if received_count < page_count as usize {
            let next_page_index = u32::try_from(received_count).map_err(|_| {
                WorkspaceConvergenceError::Inconsistent(
                    "membership history page count exceeds the protocol range".to_owned(),
                )
            })?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Continue {
                    transfer_id,
                    next_page_index,
                },
            ));
        }
        let pages = state
            .pending_membership_history_transfers
            .get(source_device_id)
            .map(|transfer| transfer.pages.clone())
            .unwrap_or_default();
        let incoming = match VersionedMembershipHistory::import_exchange_pages_v2(
            &pages,
            self.deps.historical_membership_signatures.as_ref(),
        ) {
            Ok(history) if history.lineage_id() == state.space_lineage => history,
            _ => {
                state
                    .pending_membership_history_transfers
                    .remove(source_device_id);
                self.persist(state).await?;
                return Ok(MembershipHistoryMessage::AckV2(
                    MembershipHistoryV2Ack::Invalid,
                ));
            }
        };
        let candidates = [source_device_id.clone()];
        let Some(source_member) = incoming.member_for_device(source_device_id, &candidates) else {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        };

        let current_encoded = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission::map_repository_error)?;
        if current_encoded.as_deref()
            == Some(
                incoming
                    .encode_persisted_v2()
                    .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
                    .as_slice(),
            )
        {
            state
                .pending_membership_history_transfers
                .remove(source_device_id);
            self.update_peer_history_relationship(
                state,
                source_device_id.clone(),
                MembershipHistoryRelationship::Consistent,
                now_ms,
            )?;
            self.persist(state).await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Consistent,
            ));
        }

        let current = current_encoded
            .as_deref()
            .map(|encoded| {
                VersionedMembershipHistory::decode_persisted_v2(
                    encoded,
                    self.deps.historical_membership_signatures.as_ref(),
                )
            })
            .transpose()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let source_is_allowed = current.as_ref().map_or_else(
            || incoming.active_members().contains(&source_member),
            |current| {
                incoming.active_members().contains(&source_member)
                    && incoming.is_authorized_active_member_extension_of(current, source_member)
                    || incoming.is_authorized_decision_delivery_of(current, source_member)
            },
        );
        if !source_is_allowed {
            state
                .pending_membership_history_transfers
                .remove(source_device_id);
            self.persist(state).await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let mut merged = match current {
            Some(current) => current,
            None => incoming.clone(),
        };
        let changed = if current_encoded.is_some() {
            merged
                .merge_remote_history(
                    &incoming,
                    own_instance,
                    self.deps.historical_membership_signatures.as_ref(),
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        } else {
            true
        };
        let replacement = merged
            .encode_persisted_v2()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        self.deps
            .admission_attempts
            .compare_and_replace_membership_history_v2(current_encoded.as_deref(), &replacement)
            .await
            .map_err(admission::map_repository_error)?;
        for member in merged.active_members() {
            if let Some(facts) = merged.admission_facts_for(member) {
                self.save_member_facts(facts, now_ms).await?;
            }
        }
        let relationship = if merged.pending_removal_decision(own_instance).is_some() {
            MembershipHistoryRelationship::PendingRemovalDecision
        } else if merged.removal_choices_diverge(own_instance, source_member) {
            MembershipHistoryRelationship::Diverged
        } else {
            MembershipHistoryRelationship::Consistent
        };
        self.update_peer_history_relationship(
            state,
            source_device_id.clone(),
            relationship,
            now_ms,
        )?;
        state
            .pending_membership_history_transfers
            .remove(source_device_id);
        self.persist(state).await?;
        self.publish(state);
        self.notify();
        Ok(MembershipHistoryMessage::AckV2(if changed {
            MembershipHistoryV2Ack::UpdatesApplied
        } else {
            MembershipHistoryV2Ack::Consistent
        }))
    }

    /// Persist the local user's decision for one pending remote removal.
    /// The only caller-controlled facts are the opaque pending identifier and
    /// accept/reject choice; this owner derives and signs every other field.
    /// Reconcile the local member history with every applied peer before an
    /// admission is committed. The checks remain sequential, but share one
    /// bounded budget so offline peers cannot make an admission wait once per
    /// device. The exchange is the same one used by the runtime when a peer
    /// becomes reachable, so admission cannot revive the superseded recovery
    /// channel or use a second membership source.
    pub async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
        const PRE_ADMISSION_HISTORY_SYNC_BUDGET: std::time::Duration =
            std::time::Duration::from_secs(10);

        let history_candidates = {
            let _guard = self.state_lock.lock().await;
            let state = self.load_state().await?;
            if state.removed {
                return Ok(());
            }
            state.membership_reconciliation.as_ref().map(|history| {
                history
                    .effective_members()
                    .into_iter()
                    .filter_map(|member| history.device_for_member(&member))
                    .filter(|device| *device != self.deps.own_device)
                    .collect::<Vec<_>>()
            })
        };
        let mut candidates = if let Some(candidates) = history_candidates {
            candidates
        } else {
            self.deps
                .peer_addr_repo
                .list()
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?
                .into_iter()
                .map(|record| record.device_id)
                .filter(|device| *device != self.deps.own_device)
                .collect::<Vec<_>>()
        };
        candidates.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidates.dedup();
        let deadline = tokio::time::Instant::now() + PRE_ADMISSION_HISTORY_SYNC_BUDGET;
        for device in candidates {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = tokio::time::timeout(
                remaining,
                self.reconcile_membership_history_with_peer(&device),
            )
            .await;
        }
        Ok(())
    }
}

#[async_trait]
impl MembershipHistoryExchangeEndpointPort for WorkspaceMembership {
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.handle_membership_history(source_device_id, message)
            .await
            .map_err(|_| MembershipHistoryExchangeError::Rejected)
    }
}
