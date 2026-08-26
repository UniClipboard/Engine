use std::sync::Arc;

use uc_core::membership::{
    MembershipHistoryMessage, MembershipHistoryRelationship, MembershipHistoryV2Ack,
    MembershipOperationV2, VersionedMembershipHistory, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
};

use crate::space::membership::{
    InboundMembershipTransfer as LedgerInboundTransfer, LoadedMembershipLedger,
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    PendingMembershipEffect,
};

use super::{AuthenticatedMember, HandleMembershipHistoryMessageError};

const MAX_MEMBERSHIP_TRANSFER_SIZE: usize = MAX_MEMBERSHIP_HISTORY_FRAME_SIZE * 4;

pub(crate) struct HandleMembershipHistoryMessageUseCase {
    ledger: Arc<MembershipLedger>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl HandleMembershipHistoryMessageUseCase {
    pub(crate) fn new(ledger: Arc<MembershipLedger>) -> Self {
        Self {
            ledger,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        source: &AuthenticatedMember,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let MembershipHistoryMessage::HistoryPageV2(page) = message else {
            return Err(HandleMembershipHistoryMessageError::Rejected);
        };
        if page.validate_envelope().is_err()
            || postcard::to_stdvec(&page)
                .map(|bytes| bytes.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE)
                .unwrap_or(true)
        {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let source_device_id = source.device_id().clone();
        if snapshot
            .history()
            .and_then(|history| history.effective_member_for_device(&source_device_id))
            .is_none()
        {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let transfer_id = page.transfer_id();
        if let Some(ack) = snapshot
            .record()
            .completed_inbound_transfers
            .get(&(source_device_id.clone(), transfer_id))
        {
            return Ok(MembershipHistoryMessage::AckV2(*ack));
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
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let expected_index = u32::try_from(transfer.pages.len())
            .map_err(|_| HandleMembershipHistoryMessageError::RecoveryRequired)?;
        if page_index < expected_index {
            if transfer.pages.get(&page_index) == Some(&page) {
                return Ok(MembershipHistoryMessage::AckV2(
                    MembershipHistoryV2Ack::Continue {
                        transfer_id,
                        next_page_index: expected_index,
                    },
                ));
            }
            self.commit_invalid_transfer(&snapshot, source_device_id, transfer_id)
                .await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        if page_index > expected_index {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Continue {
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
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
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
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Continue {
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
        let (_, ack) = self
            .ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, current, verifier| {
                    let incoming = match VersionedMembershipHistory::import_exchange_pages_v2(
                        &pages, verifier,
                    ) {
                        Ok(incoming) if incoming.lineage_id() == current.lineage_id() => incoming,
                        _ => {
                            mark_invalid(record, &source_device_id, transfer_id);
                            return Ok(MembershipHistoryV2Ack::Invalid);
                        }
                    };
                    let candidates = [source_device_id.clone()];
                    let Some(source_member) =
                        incoming.member_for_device(&source_device_id, &candidates)
                    else {
                        mark_invalid(record, &source_device_id, transfer_id);
                        return Ok(MembershipHistoryV2Ack::Invalid);
                    };
                    let incoming_bytes = incoming
                        .encode_persisted_v2()
                        .map_err(|_| MembershipLedgerError::Corrupt)?;
                    let current_bytes = current
                        .encode_persisted_v2()
                        .map_err(|_| MembershipLedgerError::Corrupt)?;
                    let members_before_merge = current.effective_members();
                    let ack = if incoming_bytes == current_bytes {
                        MembershipHistoryV2Ack::Consistent
                    } else if incoming.active_members().contains(&source_member)
                        && (incoming
                            .is_authorized_active_member_extension_of(current, source_member)
                            || incoming.is_authorized_decision_delivery_of(current, source_member))
                    {
                        match current.merge_remote_history(&incoming, local_member, verifier) {
                            Ok(true) => {
                                record_new_membership_effects(
                                    record,
                                    current,
                                    &members_before_merge,
                                )?;
                                MembershipHistoryV2Ack::UpdatesApplied
                            }
                            Ok(false) => MembershipHistoryV2Ack::Consistent,
                            Err(_) => MembershipHistoryV2Ack::Invalid,
                        }
                    } else {
                        MembershipHistoryV2Ack::Invalid
                    };
                    let relationship = match ack {
                        MembershipHistoryV2Ack::Consistent
                        | MembershipHistoryV2Ack::UpdatesApplied => {
                            if current.pending_removal_decision(local_member).is_some() {
                                MembershipHistoryRelationship::PendingRemovalDecision
                            } else if current.removal_choices_diverge(local_member, source_member) {
                                MembershipHistoryRelationship::Diverged
                            } else {
                                MembershipHistoryRelationship::Consistent
                            }
                        }
                        MembershipHistoryV2Ack::Diverged => MembershipHistoryRelationship::Diverged,
                        MembershipHistoryV2Ack::Invalid
                        | MembershipHistoryV2Ack::Continue { .. } => {
                            MembershipHistoryRelationship::Invalid
                        }
                    };
                    record.inbound_transfers.remove(&source_device_id);
                    record
                        .completed_inbound_transfers
                        .insert((source_device_id.clone(), transfer_id), ack);
                    let peer = record
                        .peer_reconciliation
                        .get_mut(&source_device_id)
                        .ok_or(MembershipLedgerError::Corrupt)?;
                    peer.relationship = relationship;
                    peer.confirmed_position = current.current_position().ok();
                    Ok(ack)
                },
            )
            .await
            .map_err(map_ledger_error)?;
        Ok(MembershipHistoryMessage::AckV2(ack))
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

fn mark_invalid(
    record: &mut crate::space::membership::LoadedMembershipLedger,
    source_device_id: &uc_core::ids::DeviceId,
    transfer_id: [u8; 32],
) {
    record.inbound_transfers.remove(source_device_id);
    record.completed_inbound_transfers.insert(
        (source_device_id.clone(), transfer_id),
        MembershipHistoryV2Ack::Invalid,
    );
    if let Some(peer) = record.peer_reconciliation.get_mut(source_device_id) {
        peer.relationship = MembershipHistoryRelationship::Invalid;
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
