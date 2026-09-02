use std::sync::Arc;

use uc_core::membership::{
    plan_membership_history_reconciliation, MembershipConflictEvidenceRequestV3,
    MembershipDecisionV2, MembershipEventV2, MembershipHistoryAckV3, MembershipHistoryMessage,
    MembershipHistoryReconciliationPlan, MembershipHistoryRelationship,
    MembershipHistorySuffixRequestV3, MembershipOperationV2, VersionedMembershipHistory,
    MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
};

use crate::space::membership::{
    InboundMembershipTransfer as LedgerInboundTransfer, LoadedMembershipLedger,
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    PeerReconciliationRecord, PendingMembershipEffect, WakeSpaceMembershipMaintenancePort,
};

use super::{AuthenticatedMember, HandleMembershipHistoryMessageError};

const MAX_MEMBERSHIP_TRANSFER_SIZE: usize = MAX_MEMBERSHIP_HISTORY_FRAME_SIZE * 4;
pub(super) const MAX_COMPLETED_INBOUND_TRANSFERS: usize = 256;

pub(crate) struct HandleMembershipHistoryMessageUseCase {
    ledger: Arc<MembershipLedger>,
    execution_lock: tokio::sync::Mutex<()>,
    maintenance_wake: Option<Arc<dyn WakeSpaceMembershipMaintenancePort>>,
}

impl HandleMembershipHistoryMessageUseCase {
    pub(crate) fn new(ledger: Arc<MembershipLedger>) -> Self {
        Self {
            ledger,
            execution_lock: tokio::sync::Mutex::new(()),
            maintenance_wake: None,
        }
    }

    pub(crate) fn new_with_wake(
        ledger: Arc<MembershipLedger>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            ledger,
            execution_lock: tokio::sync::Mutex::new(()),
            maintenance_wake: Some(maintenance_wake),
        }
    }

    fn wake_after_history_change(&self, changed: bool) {
        if changed {
            if let Some(wake) = self.maintenance_wake.as_ref() {
                // 历史、effects 与 fan-out 欠账已经原子提交；wake 只负责降低恢复延迟。
                wake.wake();
            }
        }
    }

    pub(crate) async fn execute(
        &self,
        source: &AuthenticatedMember,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let page = match message {
            MembershipHistoryMessage::SummaryV3(summary) => {
                let snapshot = self
                    .ledger
                    .load_verified()
                    .await
                    .map_err(map_ledger_error)?;
                let history = snapshot
                    .history()
                    .ok_or(HandleMembershipHistoryMessageError::RecoveryRequired)?;
                let sender_claim_matches_connection =
                    &summary.sender_admission.device_id == source.device_id();
                let sender_is_current = history
                    .effective_member_for_device(source.device_id())
                    .and_then(|member| history.admission_facts_for(member))
                    == Some(&summary.sender_admission);
                if !sender_claim_matches_connection || summary.lineage_id != history.lineage_id() {
                    return Ok(MembershipHistoryMessage::AckV3(
                        MembershipHistoryAckV3::Invalid,
                    ));
                }
                let current_position = history
                    .current_position()
                    .map_err(|_| HandleMembershipHistoryMessageError::RecoveryRequired)?;
                let plan = plan_membership_history_reconciliation(
                    history.lineage_id(),
                    &current_position,
                    &summary.lineage_id,
                    &summary.current_position,
                    history.contains_strict_ancestor_position(&summary.current_position),
                );
                tracing::debug!(
                    plan = reconciliation_plan_kind(plan),
                    "成员历史摘要完成关系规划"
                );
                return match plan {
                    MembershipHistoryReconciliationPlan::Noop
                    | MembershipHistoryReconciliationPlan::OfferSuffix => {
                        if !sender_is_current {
                            return Ok(MembershipHistoryMessage::AckV3(
                                MembershipHistoryAckV3::Invalid,
                            ));
                        }
                        // OfferSuffix 先确认远端真实祖先；本机持久欠账会驱动反向发送。
                        Ok(MembershipHistoryMessage::AckV3(
                            MembershipHistoryAckV3::Confirmed {
                                transfer_id: summary.transfer_id,
                                confirmed_position: summary.current_position,
                            },
                        ))
                    }
                    MembershipHistoryReconciliationPlan::RequestSuffix => {
                        Ok(MembershipHistoryMessage::RequestSuffixV3(
                            MembershipHistorySuffixRequestV3 {
                                transfer_id: summary.transfer_id,
                                known_position: current_position,
                            },
                        ))
                    }
                    MembershipHistoryReconciliationPlan::Diverged => {
                        Ok(MembershipHistoryMessage::RequestConflictEvidenceV3(
                            MembershipConflictEvidenceRequestV3 {
                                transfer_id: summary.transfer_id,
                            },
                        ))
                    }
                    MembershipHistoryReconciliationPlan::Invalid => Ok(
                        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid),
                    ),
                };
            }
            MembershipHistoryMessage::SuffixPageV3(page) => page,
            MembershipHistoryMessage::ConflictEvidenceV3(evidence) => {
                return self.receive_conflict_evidence(source, evidence).await;
            }
            MembershipHistoryMessage::RestrictedEventV3(event) => {
                return self.receive_restricted_event(source, event).await;
            }
            MembershipHistoryMessage::RestrictedDecisionV3(decision) => {
                return self.receive_restricted_decision(source, decision).await;
            }
            MembershipHistoryMessage::RequestSuffixV3(_)
            | MembershipHistoryMessage::RequestConflictEvidenceV3(_)
            | MembershipHistoryMessage::AckV3(_) => {
                return Err(HandleMembershipHistoryMessageError::Rejected);
            }
        };
        if page.validate_envelope().is_err()
            || postcard::to_stdvec(&page)
                .map(|bytes| bytes.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE)
                .unwrap_or(true)
        {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let source_device_id = source.device_id().clone();
        let sender_was_removed = snapshot.history().is_some_and(|history| {
            history.admission_facts_for(page.sender_admission().member_instance)
                == Some(page.sender_admission())
                && history
                    .effective_member_for_device(&source_device_id)
                    .is_none()
        });
        if page.sender_admission().device_id != source_device_id || sender_was_removed {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let transfer_id = page.transfer_id();
        if let Some(ack) = snapshot
            .record()
            .completed_inbound_transfers
            .get(&(source_device_id.clone(), transfer_id))
        {
            tracing::debug!(
                ack_kind = history_ack_kind(ack),
                "成员历史入站传输命中幂等 ACK"
            );
            return Ok(MembershipHistoryMessage::AckV3(ack.clone()));
        }
        let page_index = page.page_index();
        let page_count = page.page_count();
        let mut transfer = snapshot
            .record()
            .inbound_transfers
            .get(&source_device_id)
            .cloned()
            .unwrap_or_else(|| LedgerInboundTransfer {
                source_device_id: source_device_id.clone(),
                transfer_id,
                page_count,
                pages: Default::default(),
                total_bytes: 0,
            });
        if transfer.transfer_id != transfer_id || transfer.page_count != page_count {
            self.commit_invalid_transfer(&snapshot, source_device_id, transfer_id)
                .await?;
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let expected_index = u32::try_from(transfer.pages.len())
            .map_err(|_| HandleMembershipHistoryMessageError::RecoveryRequired)?;
        if page_index < expected_index {
            if transfer.pages.get(&page_index) == Some(&page) {
                return Ok(MembershipHistoryMessage::AckV3(
                    MembershipHistoryAckV3::Continue {
                        transfer_id,
                        next_page_index: expected_index,
                    },
                ));
            }
            self.commit_invalid_transfer(&snapshot, source_device_id, transfer_id)
                .await?;
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        if page_index > expected_index {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Continue {
                    transfer_id,
                    next_page_index: expected_index,
                },
            ));
        }
        transfer.total_bytes = transfer
            .total_bytes
            .checked_add(
                postcard::to_stdvec(&page)
                    .map_err(|_| HandleMembershipHistoryMessageError::Rejected)?
                    .len(),
            )
            .ok_or(HandleMembershipHistoryMessageError::RecoveryRequired)?;
        if transfer.total_bytes > MAX_MEMBERSHIP_TRANSFER_SIZE {
            self.commit_invalid_transfer(&snapshot, source_device_id, transfer_id)
                .await?;
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        transfer.pages.insert(page_index, page);
        if transfer.pages.len() < page_count as usize {
            let next_page_index = u32::try_from(transfer.pages.len())
                .map_err(|_| HandleMembershipHistoryMessageError::RecoveryRequired)?;
            let transfer_for_commit = transfer;
            self.ledger
                .compare_and_commit(|record| {
                    record
                        .inbound_transfers
                        .insert(source_device_id.clone(), transfer_for_commit);
                    Ok(())
                })
                .await
                .map_err(map_ledger_error)?;
            tracing::debug!(
                received_page_count = next_page_index,
                page_count,
                "成员历史入站后缀已持久等待后续页"
            );
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Continue {
                    transfer_id,
                    next_page_index,
                },
            ));
        }

        let pages = transfer.pages.values().cloned().collect::<Vec<_>>();
        let expected_revision = snapshot.record().revision;
        let expected_history_digest = snapshot.history_digest();
        let local_member = snapshot
            .record()
            .local_member_instance
            .ok_or(HandleMembershipHistoryMessageError::RecoveryRequired)?;
        let (_, (ack, new_effect_count, pending_peer_count, sender_is_bound)) = self
            .ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, current, verifier| {
                    let members_before_merge = current.effective_members();
                    let effects_before = record.pending_effects.len();
                    let sender_is_bound = pages
                        .first()
                        .is_some_and(|page| page.sender_admission().device_id == source_device_id);
                    // 不可信后缀先在副本上完整验证；失败时绝不能把部分事件写入账本。
                    let mut candidate = current.clone();
                    let ack = match sender_is_bound
                        .then(|| candidate.apply_suffix_pages_v3(&pages, local_member, verifier))
                    {
                        Some(Ok(true)) => {
                            *current = candidate;
                            record_new_membership_effects(record, current, &members_before_merge)?;
                            let confirmed_position = current
                                .current_position()
                                .map_err(|_| MembershipLedgerError::Corrupt)?;
                            MembershipHistoryAckV3::Confirmed {
                                transfer_id,
                                confirmed_position,
                            }
                        }
                        Some(Ok(false)) | Some(Err(_)) | None => MembershipHistoryAckV3::Invalid,
                    };
                    let relationship = match &ack {
                        MembershipHistoryAckV3::Confirmed { .. } => {
                            if current.pending_removal_decision(local_member).is_some() {
                                MembershipHistoryRelationship::PendingRemovalDecision
                            } else {
                                MembershipHistoryRelationship::Consistent
                            }
                        }
                        MembershipHistoryAckV3::Diverged => MembershipHistoryRelationship::Diverged,
                        _ => MembershipHistoryRelationship::Invalid,
                    };
                    record.inbound_transfers.remove(&source_device_id);
                    remember_completed_inbound_transfer(
                        record,
                        source_device_id.clone(),
                        transfer_id,
                        ack.clone(),
                    );
                    let peer = record
                        .peer_reconciliation
                        .entry(source_device_id.clone())
                        .or_insert_with(|| PeerReconciliationRecord {
                            peer_device_id: source_device_id.clone(),
                            relationship: MembershipHistoryRelationship::Unknown,
                            confirmed_position: None,
                            sync_state: Default::default(),
                            restricted_delivery: Vec::new(),
                            updated_at_ms: 0,
                        });
                    peer.relationship = relationship;
                    if matches!(ack, MembershipHistoryAckV3::Confirmed { .. }) {
                        peer.confirmed_position = current.current_position().ok();
                    }
                    let new_effect_count =
                        record.pending_effects.len().saturating_sub(effects_before);
                    let current_position = current.current_position().ok();
                    let pending_peer_count = record
                        .peer_reconciliation
                        .values()
                        .filter(|peer| {
                            peer.confirmed_position.as_ref() != current_position.as_ref()
                        })
                        .count();
                    Ok((ack, new_effect_count, pending_peer_count, sender_is_bound))
                },
            )
            .await
            .map_err(map_ledger_error)?;
        tracing::debug!(
            ack_kind = history_ack_kind(&ack),
            sender_is_bound,
            page_count,
            new_effect_count,
            pending_peer_count,
            "成员历史入站后缀完成原子处理"
        );
        self.wake_after_history_change(matches!(ack, MembershipHistoryAckV3::Confirmed { .. }));
        Ok(MembershipHistoryMessage::AckV3(ack))
    }

    async fn receive_conflict_evidence(
        &self,
        source: &AuthenticatedMember,
        evidence: uc_core::membership::MembershipConflictEvidenceV3,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        if evidence.pages.is_empty()
            || postcard::to_stdvec(&MembershipHistoryMessage::ConflictEvidenceV3(
                evidence.clone(),
            ))
            .map(|bytes| bytes.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE)
            .unwrap_or(true)
        {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let _guard = self.execution_lock.lock().await;
        let Some(pages) = self
            .ledger
            .exchange_conflict_evidence(source.device_id(), &evidence)
            .await
            .map_err(map_ledger_error)?
        else {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        };
        let transfer_id = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?
            .history()
            .ok_or(HandleMembershipHistoryMessageError::RecoveryRequired)?
            .current_position()
            .map_err(|_| HandleMembershipHistoryMessageError::RecoveryRequired)?
            .history_digest;
        Ok(MembershipHistoryMessage::ConflictEvidenceV3(
            uc_core::membership::MembershipConflictEvidenceV3 { transfer_id, pages },
        ))
    }

    async fn receive_restricted_event(
        &self,
        source: &AuthenticatedMember,
        event: MembershipEventV2,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let source_device_id = source.device_id().clone();
        let source_member = snapshot
            .history()
            .and_then(|history| history.effective_member_for_device(&source_device_id));
        if source_member != Some(event.author_member_instance_id) {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let expected_revision = snapshot.record().revision;
        let expected_history_digest = snapshot.history_digest();
        let (_, ack) = self
            .ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, history, verifier| {
                    let local_member = record
                        .local_member_instance
                        .ok_or(MembershipLedgerError::Corrupt)?;
                    let members_before = history.effective_members();
                    let ack = match history.verify_and_receive_remote_event_for_local_member(
                        event,
                        local_member,
                        verifier,
                    ) {
                        Ok(uc_core::membership::MembershipHistoryV2ReceiveOutcome::Applied) => {
                            record_new_membership_effects(record, history, &members_before)?;
                            MembershipHistoryAckV3::RestrictedApplied
                        }
                        Ok(
                            uc_core::membership::MembershipHistoryV2ReceiveOutcome::AlreadyKnown,
                        ) => MembershipHistoryAckV3::RestrictedConsistent,
                        Ok(uc_core::membership::MembershipHistoryV2ReceiveOutcome::Diverged)
                        | Err(_) => MembershipHistoryAckV3::Invalid,
                    };
                    update_restricted_relationship(record, &source_device_id, history, &ack);
                    Ok(ack)
                },
            )
            .await
            .map_err(map_ledger_error)?;
        self.wake_after_history_change(matches!(ack, MembershipHistoryAckV3::RestrictedApplied));
        Ok(MembershipHistoryMessage::AckV3(ack))
    }

    async fn receive_restricted_decision(
        &self,
        source: &AuthenticatedMember,
        decision: MembershipDecisionV2,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let source_device_id = source.device_id().clone();
        let source_member = snapshot.history().and_then(|history| {
            history.member_for_device(&source_device_id, std::slice::from_ref(&source_device_id))
        });
        if source_member != Some(decision.decided_by_member_instance_id) {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let expected_revision = snapshot.record().revision;
        let expected_history_digest = snapshot.history_digest();
        let (_, ack) = self
            .ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, history, verifier| {
                    let members_before = history.effective_members();
                    let ack = match history.verify_and_record_peer_decision(decision, verifier) {
                        Ok(_) => {
                            record_new_membership_effects(record, history, &members_before)?;
                            MembershipHistoryAckV3::RestrictedApplied
                        }
                        Err(_) => MembershipHistoryAckV3::Invalid,
                    };
                    update_restricted_relationship(record, &source_device_id, history, &ack);
                    Ok(ack)
                },
            )
            .await
            .map_err(map_ledger_error)?;
        self.wake_after_history_change(matches!(ack, MembershipHistoryAckV3::RestrictedApplied));
        Ok(MembershipHistoryMessage::AckV3(ack))
    }

    async fn commit_invalid_transfer(
        &self,
        snapshot: &crate::space::membership::VerifiedMembershipLedger,
        source_device_id: uc_core::ids::DeviceId,
        transfer_id: [u8; 32],
    ) -> Result<(), HandleMembershipHistoryMessageError> {
        let expected_revision = snapshot.record().revision;
        let expected_history_digest = snapshot.history_digest();
        self.ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, _history, _verifier| {
                    mark_invalid(record, &source_device_id, transfer_id);
                    Ok(())
                },
            )
            .await
            .map_err(map_ledger_error)?;
        Ok(())
    }
}

fn reconciliation_plan_kind(plan: MembershipHistoryReconciliationPlan) -> &'static str {
    match plan {
        MembershipHistoryReconciliationPlan::Noop => "noop",
        MembershipHistoryReconciliationPlan::OfferSuffix => "offer_suffix",
        MembershipHistoryReconciliationPlan::RequestSuffix => "request_suffix",
        MembershipHistoryReconciliationPlan::Diverged => "diverged",
        MembershipHistoryReconciliationPlan::Invalid => "invalid",
    }
}

fn history_ack_kind(ack: &MembershipHistoryAckV3) -> &'static str {
    match ack {
        MembershipHistoryAckV3::Continue { .. } => "continue",
        MembershipHistoryAckV3::Confirmed { .. } => "confirmed",
        MembershipHistoryAckV3::RestrictedApplied => "restricted_applied",
        MembershipHistoryAckV3::RestrictedConsistent => "restricted_consistent",
        MembershipHistoryAckV3::Diverged => "diverged",
        MembershipHistoryAckV3::Invalid => "invalid",
    }
}

fn update_restricted_relationship(
    record: &mut LoadedMembershipLedger,
    source_device_id: &uc_core::ids::DeviceId,
    history: &VersionedMembershipHistory,
    ack: &MembershipHistoryAckV3,
) {
    if let Some(peer) = record.peer_reconciliation.get_mut(source_device_id) {
        peer.relationship = match ack {
            MembershipHistoryAckV3::RestrictedConsistent
            | MembershipHistoryAckV3::RestrictedApplied => {
                MembershipHistoryRelationship::Consistent
            }
            MembershipHistoryAckV3::Diverged => MembershipHistoryRelationship::Diverged,
            _ => MembershipHistoryRelationship::Invalid,
        };
        // 受限事件 ACK 只确认该事件，不能证明来源端拥有本机完整历史位置。
        let _ = history;
    }
}

fn mark_invalid(
    record: &mut crate::space::membership::LoadedMembershipLedger,
    source_device_id: &uc_core::ids::DeviceId,
    transfer_id: [u8; 32],
) {
    record.inbound_transfers.remove(source_device_id);
    remember_completed_inbound_transfer(
        record,
        source_device_id.clone(),
        transfer_id,
        MembershipHistoryAckV3::Invalid,
    );
    if let Some(peer) = record.peer_reconciliation.get_mut(source_device_id) {
        peer.relationship = MembershipHistoryRelationship::Invalid;
    }
}

pub(super) fn remember_completed_inbound_transfer(
    record: &mut LoadedMembershipLedger,
    source_device_id: uc_core::ids::DeviceId,
    transfer_id: [u8; 32],
    ack: MembershipHistoryAckV3,
) {
    let latest_key = (source_device_id, transfer_id);
    record
        .completed_inbound_transfers
        .insert(latest_key.clone(), ack);
    while record.completed_inbound_transfers.len() > MAX_COMPLETED_INBOUND_TRANSFERS {
        let Some(evicted_key) = record
            .completed_inbound_transfers
            .keys()
            .find(|key| *key != &latest_key)
            .cloned()
        else {
            break;
        };
        record.completed_inbound_transfers.remove(&evicted_key);
    }
}

fn record_new_membership_effects(
    record: &mut LoadedMembershipLedger,
    history: &VersionedMembershipHistory,
    members_before_merge: &std::collections::BTreeSet<uc_core::membership::MemberInstanceId>,
) -> Result<(), MembershipLedgerError> {
    let members_after_merge = history.effective_members();
    let added = members_after_merge
        .difference(members_before_merge)
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let removed = members_before_merge
        .difference(&members_after_merge)
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for member in &members_after_merge {
        let Some(facts) = history.admission_facts_for(*member) else {
            return Err(MembershipLedgerError::Corrupt);
        };
        if record.local_device_id.as_ref() == Some(&facts.device_id) {
            continue;
        }
        record
            .peer_reconciliation
            .entry(facts.device_id.clone())
            .or_insert_with(|| crate::space::membership::PeerReconciliationRecord {
                peer_device_id: facts.device_id.clone(),
                relationship: MembershipHistoryRelationship::Unknown,
                confirmed_position: None,
                sync_state: crate::space::membership::PeerHistorySyncState {
                    pending_since_revision: Some(record.revision.saturating_add(1)),
                    ..Default::default()
                },
                restricted_delivery: Vec::new(),
                updated_at_ms: 0,
            });
    }
    let mut event_id = history.current_head();
    while let Some(current_event_id) = event_id {
        let Some(event) = history.event(current_event_id) else {
            break;
        };
        let (kind, member) = match &event.operation {
            MembershipOperationV2::AddDevice { admission }
                if added.contains(&admission.facts.member_instance) =>
            {
                (
                    MembershipEffectKind::AddDevice,
                    admission.facts.member_instance,
                )
            }
            MembershipOperationV2::RemoveDevice { member } if removed.contains(member) => {
                (MembershipEffectKind::RemoveDevice, *member)
            }
            MembershipOperationV2::AddDevice { .. }
            | MembershipOperationV2::RemoveDevice { .. } => {
                event_id = event.parent_event_id;
                continue;
            }
        };
        let device_id = history
            .admission_facts_for(member)
            .map(|facts| facts.device_id.clone())
            .ok_or(MembershipLedgerError::Corrupt)?;
        record
            .pending_effects
            .entry(*current_event_id.as_bytes())
            .or_insert(PendingMembershipEffect {
                event_id: *current_event_id.as_bytes(),
                kind,
                phase: MembershipEffectPhase::Prepared,
                affected_device_ids: vec![device_id],
                payload: postcard::to_stdvec(event).map_err(|_| MembershipLedgerError::Corrupt)?,
            });
        event_id = event.parent_event_id;
    }
    if record
        .pending_effects
        .values()
        .filter(|effect| effect.phase == MembershipEffectPhase::Prepared)
        .flat_map(|effect| effect.affected_device_ids.iter())
        .filter(|device_id| {
            history
                .member_for_device(device_id, std::slice::from_ref(device_id))
                .is_some_and(|member| added.contains(&member) || removed.contains(&member))
        })
        .count()
        < added.len() + removed.len()
    {
        return Err(MembershipLedgerError::Corrupt);
    }
    Ok(())
}

fn map_ledger_error(error: MembershipLedgerError) -> HandleMembershipHistoryMessageError {
    match error {
        MembershipLedgerError::Locked => HandleMembershipHistoryMessageError::Locked,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            HandleMembershipHistoryMessageError::RecoveryRequired
        }
        MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable => {
            HandleMembershipHistoryMessageError::Unavailable
        }
    }
}

#[async_trait::async_trait]
impl uc_core::membership::MembershipHistoryExchangeEndpointPort
    for HandleMembershipHistoryMessageUseCase
{
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, uc_core::membership::MembershipHistoryExchangeError> {
        self.execute(&AuthenticatedMember::new(source_device_id.clone()), message)
            .await
            .map_err(|_| uc_core::membership::MembershipHistoryExchangeError::Rejected)
    }
}
