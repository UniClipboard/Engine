use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    plan_membership_history_reconciliation, LedgerInput, MembershipConflictEvidenceRequestV3,
    MembershipConflictPolicy, MembershipDecisionV2, MembershipEventV2, MembershipHistoryAckV3,
    MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryMessage, MembershipHistoryReconciliationPlan,
    MembershipHistorySuffixRequestV3, MembershipHistoryV2Error, MembershipHistoryV2ReceiveOutcome,
    PeerEvidence, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
};

use crate::space::membership::{
    ledger_error, AcquireSpaceWorkPermitPort, InboundMembershipTransfer,
    MembershipHistoryExchangeRecord, MembershipLedgerError, MembershipOwner,
    QuerySpaceWorkModeError, ReconcileMembershipEvidenceUseCase, SpaceWorkMode,
};

use super::{AuthenticatedMember, HandleMembershipHistoryMessageError};

pub(super) const MAX_COMPLETED_INBOUND_TRANSFERS: usize = 256;

/// 一条已认证成员历史消息的完整处理：先持久保存结果，再回复 ACK。
pub(crate) struct HandleMembershipHistoryMessageUseCase {
    owner: Arc<MembershipOwner>,
    evidence: ReconcileMembershipEvidenceUseCase,
    execution_lock: tokio::sync::Mutex<()>,
    work_mode: Arc<dyn AcquireSpaceWorkPermitPort>,
}

impl HandleMembershipHistoryMessageUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        work_mode: Arc<dyn AcquireSpaceWorkPermitPort>,
    ) -> Self {
        Self {
            evidence: ReconcileMembershipEvidenceUseCase::new(Arc::clone(&owner)),
            owner,
            execution_lock: tokio::sync::Mutex::new(()),
            work_mode,
        }
    }

    pub(crate) async fn execute(
        &self,
        source: &AuthenticatedMember,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let work_permit = self
            .work_mode
            .acquire_space_work_permit()
            .await
            .map_err(|error| match error {
                QuerySpaceWorkModeError::Unavailable => {
                    HandleMembershipHistoryMessageError::Unavailable
                }
                QuerySpaceWorkModeError::NeedsAttention => {
                    HandleMembershipHistoryMessageError::RecoveryRequired
                }
            })?;
        match work_permit.mode() {
            SpaceWorkMode::Active => {}
            SpaceWorkMode::Pairing => {
                return Err(HandleMembershipHistoryMessageError::PairingInProgress)
            }
            SpaceWorkMode::NeedsAttention => {
                return Err(HandleMembershipHistoryMessageError::RecoveryRequired)
            }
        }
        let page = match message {
            MembershipHistoryMessage::SummaryV3(summary) => {
                let view = self.owner.load().await.map_err(map_ledger_error)?;
                let history = view.require_space().map_err(map_ledger_error)?.history();
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
            MembershipHistoryMessage::SuffixPageV4(page) => page,
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
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let space = view.require_space().map_err(map_ledger_error)?;
        let history = space.history();
        let source_device_id = *source.device_id();
        let sender_was_removed = history
            .admission_facts_for(page.sender_admission().member_instance)
            == Some(page.sender_admission())
            && history
                .effective_member_for_device(&source_device_id)
                .is_none();
        if page.sender_admission().device_id != source_device_id || sender_was_removed {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let transfer_id = page.transfer_id();
        if let Some(ack) = space
            .history_exchange()
            .completed_inbound_transfers
            .get(&(source_device_id, transfer_id))
        {
            tracing::debug!(
                ack_kind = history_ack_kind(ack),
                "成员历史入站传输命中幂等 ACK"
            );
            return Ok(MembershipHistoryMessage::AckV3(ack.clone()));
        }
        let admission = InboundMembershipTransfer::accept_page(
            space
                .history_exchange()
                .inbound_transfers
                .get(&source_device_id)
                .cloned(),
            source_device_id,
            page,
        )?;
        let transfer = match admission {
            super::transfer::PageAdmission::Rejected => {
                self.commit_invalid_transfer(source_device_id, transfer_id)
                    .await?;
                return Ok(MembershipHistoryMessage::AckV3(
                    MembershipHistoryAckV3::Invalid,
                ));
            }
            super::transfer::PageAdmission::Continue { next, changed } => {
                if let Some(transfer) = changed {
                    self.owner
                        .commit(|draft| {
                            draft
                                .history_exchange_mut()?
                                .inbound_transfers
                                .insert(source_device_id, transfer);
                            Ok(())
                        })
                        .await
                        .map_err(map_ledger_error)?;
                    tracing::debug!(
                        received_page_count = next,
                        "成员历史入站后缀已持久等待后续页"
                    );
                }
                return Ok(MembershipHistoryMessage::AckV3(
                    MembershipHistoryAckV3::Continue {
                        transfer_id,
                        next_page_index: next,
                    },
                ));
            }
            super::transfer::PageAdmission::Complete(transfer) => transfer,
        };

        let page_count = transfer.page_count;
        let pages = transfer.pages.values().cloned().collect::<Vec<_>>();
        let local_member = space.local_member();
        let verifier = self.owner.verifier_handle();
        let committed = self
            .owner
            .commit(move |draft| {
                let current = draft.require_space()?.history().clone();
                let effects_before = draft.require_space()?.ledger().unfinished_effects().count();
                let sender_is_bound = pages
                    .first()
                    .is_some_and(|page| page.sender_admission().device_id == source_device_id);
                // 不可信后缀先在副本上完整验证；失败时绝不能把部分事件写入账本。
                let mut candidate = current.clone();
                let (ack, adopted) = match sender_is_bound.then(|| {
                    candidate.apply_suffix_pages_v4(&pages, local_member, verifier.as_ref())
                }) {
                    // 发送方的当前头正是本机已拒绝的移除：双方已分叉，不再确认对方位置。
                    Some(Ok(proven_sender))
                        if MembershipConflictPolicy::local_choice_already_recorded(
                            &candidate,
                            &proven_sender,
                            local_member,
                        ) =>
                    {
                        (MembershipHistoryAckV3::Diverged, Some(candidate))
                    }
                    Some(Ok(proven_sender)) => {
                        let same_branch = MembershipConflictPolicy::branch_id(&candidate)
                            .map_err(MembershipLedgerError::corrupt_from)?
                            == MembershipConflictPolicy::branch_id(&proven_sender)
                                .map_err(MembershipLedgerError::corrupt_from)?
                            && candidate.active_members() == proven_sender.active_members();
                        let confirmed = if same_branch {
                            &proven_sender
                        } else {
                            &candidate
                        };
                        let confirmed_position = confirmed
                            .current_position()
                            .map_err(MembershipLedgerError::corrupt_from)?;
                        (
                            MembershipHistoryAckV3::Confirmed {
                                transfer_id,
                                confirmed_position,
                            },
                            Some(candidate),
                        )
                    }
                    Some(Err(
                        MembershipHistoryV2Error::IncompleteHistoryProof
                        | MembershipHistoryV2Error::HistoryPositionChanged
                        | MembershipHistoryV2Error::UnknownParent,
                    )) => (MembershipHistoryAckV3::NeedsEvidence, None),
                    Some(Err(_)) | None => (MembershipHistoryAckV3::Invalid, None),
                };
                let evidence = match &ack {
                    MembershipHistoryAckV3::Confirmed { .. } => PeerEvidence::Confirmed,
                    MembershipHistoryAckV3::NeedsEvidence => PeerEvidence::NeedsEvidence,
                    MembershipHistoryAckV3::Diverged => PeerEvidence::Diverged,
                    _ => PeerEvidence::Invalid,
                };
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: source_device_id,
                        history: adopted,
                        evidence,
                    })
                    .map_err(ledger_error)?;
                let exchange = draft.history_exchange_mut()?;
                exchange.inbound_transfers.remove(&source_device_id);
                if !matches!(ack, MembershipHistoryAckV3::NeedsEvidence) {
                    remember_completed_inbound_transfer(
                        exchange,
                        source_device_id,
                        transfer_id,
                        ack.clone(),
                    );
                }
                let space = draft.require_space()?;
                let new_effect_count = space
                    .ledger()
                    .unfinished_effects()
                    .count()
                    .saturating_sub(effects_before);
                Ok((ack, new_effect_count, sender_is_bound))
            })
            .await
            .map_err(map_ledger_error)?;
        let (ack, new_effect_count, sender_is_bound) = committed.output;
        tracing::debug!(
            ack_kind = history_ack_kind(&ack),
            sender_is_bound,
            page_count,
            new_effect_count,
            "成员历史入站后缀完成原子处理"
        );
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
        let Some(exchange) = self
            .evidence
            .execute(source.device_id(), &evidence)
            .await
            .map_err(map_ledger_error)?
        else {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        };
        Ok(MembershipHistoryMessage::ConflictEvidenceV3(
            exchange.response,
        ))
    }

    async fn receive_restricted_event(
        &self,
        source: &AuthenticatedMember,
        event: MembershipEventV2,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let _guard = self.execution_lock.lock().await;
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let source_device_id = *source.device_id();
        let source_member = view.space().and_then(|space| {
            space
                .history()
                .effective_member_for_device(&source_device_id)
        });
        if source_member != Some(event.author_member_instance_id) {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let verifier = self.owner.verifier_handle();
        let committed = self
            .owner
            .commit(move |draft| {
                let space = draft.require_space()?;
                let local_member = space.local_member();
                let mut history = space.history().clone();
                let (ack, adopted, evidence) = match history
                    .verify_and_receive_remote_event_for_local_member(
                        event,
                        local_member,
                        verifier.as_ref(),
                    ) {
                    Ok(MembershipHistoryV2ReceiveOutcome::Applied) => (
                        MembershipHistoryAckV3::RestrictedApplied,
                        Some(history),
                        PeerEvidence::Consistent,
                    ),
                    Ok(MembershipHistoryV2ReceiveOutcome::AlreadyKnown) => (
                        MembershipHistoryAckV3::RestrictedConsistent,
                        None,
                        PeerEvidence::Consistent,
                    ),
                    Ok(MembershipHistoryV2ReceiveOutcome::Diverged) | Err(_) => {
                        (MembershipHistoryAckV3::Invalid, None, PeerEvidence::Invalid)
                    }
                };
                // 受限事件 ACK 只确认该事件，不能证明来源端拥有本机完整历史位置。
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: source_device_id,
                        history: adopted,
                        evidence,
                    })
                    .map_err(ledger_error)?;
                Ok(ack)
            })
            .await
            .map_err(map_ledger_error)?;
        Ok(MembershipHistoryMessage::AckV3(committed.output))
    }

    async fn receive_restricted_decision(
        &self,
        source: &AuthenticatedMember,
        decision: MembershipDecisionV2,
    ) -> Result<MembershipHistoryMessage, HandleMembershipHistoryMessageError> {
        let _guard = self.execution_lock.lock().await;
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let source_device_id = *source.device_id();
        // 决定必须由来源设备自己的成员实例签署；同一设备重新加入后有多个实例，直接核对签署实例所属设备。
        let signer_device = view.space().and_then(|space| {
            space.history().device_for_member(
                &decision.decided_by_member_instance_id,
                std::slice::from_ref(&source_device_id),
            )
        });
        if signer_device != Some(source_device_id) {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ));
        }
        let verifier = self.owner.verifier_handle();
        let committed = self
            .owner
            .commit(move |draft| {
                let mut history = draft.require_space()?.history().clone();
                let (ack, adopted, evidence) =
                    match history.verify_and_record_peer_decision(decision, verifier.as_ref()) {
                        Ok(_) => (
                            MembershipHistoryAckV3::RestrictedApplied,
                            Some(history),
                            PeerEvidence::Consistent,
                        ),
                        Err(_) => (MembershipHistoryAckV3::Invalid, None, PeerEvidence::Invalid),
                    };
                // 已被移除的设备送来的决定只贡献已验证历史，由账本决定是否记录关系。
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: source_device_id,
                        history: adopted,
                        evidence,
                    })
                    .map_err(ledger_error)?;
                Ok(ack)
            })
            .await
            .map_err(map_ledger_error)?;
        Ok(MembershipHistoryMessage::AckV3(committed.output))
    }

    async fn commit_invalid_transfer(
        &self,
        source_device_id: DeviceId,
        transfer_id: [u8; 32],
    ) -> Result<(), HandleMembershipHistoryMessageError> {
        self.owner
            .commit(move |draft| {
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: source_device_id,
                        history: None,
                        evidence: PeerEvidence::Invalid,
                    })
                    .map_err(ledger_error)?;
                let exchange = draft.history_exchange_mut()?;
                exchange.inbound_transfers.remove(&source_device_id);
                remember_completed_inbound_transfer(
                    exchange,
                    source_device_id,
                    transfer_id,
                    MembershipHistoryAckV3::Invalid,
                );
                Ok(())
            })
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
        MembershipHistoryAckV3::NeedsEvidence => "needs_evidence",
    }
}

pub(super) fn remember_completed_inbound_transfer(
    record: &mut MembershipHistoryExchangeRecord,
    source_device_id: DeviceId,
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

fn map_ledger_error(error: MembershipLedgerError) -> HandleMembershipHistoryMessageError {
    match error {
        MembershipLedgerError::Locked => HandleMembershipHistoryMessageError::Locked,
        MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
            HandleMembershipHistoryMessageError::RecoveryRequired
        }
        MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable { .. } => {
            HandleMembershipHistoryMessageError::Unavailable
        }
    }
}

#[async_trait::async_trait]
impl MembershipHistoryExchangeEndpointPort for HandleMembershipHistoryMessageUseCase {
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let result = self
            .execute(&AuthenticatedMember::new(*source_device_id), message)
            .await;
        result.map_err(|error| match error {
            HandleMembershipHistoryMessageError::PairingInProgress => {
                MembershipHistoryExchangeError::PairingInProgress
            }
            _ => MembershipHistoryExchangeError::Rejected,
        })
    }
}
