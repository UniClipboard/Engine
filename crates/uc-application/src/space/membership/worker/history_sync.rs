//! 向选定对端同步本机成员历史：只执行交换协议，结果作为输入交回 Owner。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, StreamExt};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    ack_confirms_membership_history_target, AdmissionChangeFacts, BaseMembershipHistoryPosition,
    LedgerInput, LedgerMemberStatus, MembershipConflictEvidenceV3, MembershipHistoryAckV3,
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    MembershipHistoryRelationship, MembershipHistorySuffixPageV4, MembershipHistorySuffixRequestV3,
    MembershipHistorySummaryV3, PeerLink, PeerRelation, PeerSyncResult, VersionedMembershipHistory,
};
use uc_observability_contract::diagnostics::{
    describe_membership_conflict, MembershipRecoveryObservation, MembershipRecoveryOutcome,
};

use crate::space::membership::{
    ledger_error, MembershipLedgerError, MembershipOwner, ReconcileMembershipEvidenceUseCase,
};

/// 一轮同步的固定总预算，不按对端数量叠加。
const TOTAL_SYNC_BUDGET: Duration = Duration::from_secs(10);
pub(super) const MAX_PEERS_PER_ROUND: usize = 8;
const MAX_CONCURRENT_PEERS: usize = 4;

/// 在成员历史完成验证后，刷新已认证成员的可复用网络地址。
#[async_trait]
pub trait RefreshVerifiedPeerAddressPort: Send + Sync {
    async fn refresh_verified_peer_address(&self, peer: &DeviceId);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct HistorySyncReport {
    pub(super) completed_peer_count: usize,
    pub(super) deferred_peer_count: usize,
    pub(super) stable_failure_count: usize,
}

#[derive(Clone, Copy)]
enum HistoryProofRequirement {
    Incremental,
    Complete,
}

/// 一个对端本轮交换的结论。
enum PeerExchange {
    /// 交给账本的同步结果。
    Finished(PeerSyncResult),
    /// 完整证据核对已由证据流程记录为分叉，本轮不再提交结果。
    DivergenceRecorded,
}

pub(super) struct HistorySynchronizer {
    owner: Arc<MembershipOwner>,
    evidence: ReconcileMembershipEvidenceUseCase,
    transport: Arc<dyn MembershipHistoryExchangePort>,
    address_refresh: Arc<dyn RefreshVerifiedPeerAddressPort>,
    peer_locks: tokio::sync::Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
}

struct SyncContext {
    history: VersionedMembershipHistory,
    sender: AdmissionChangeFacts,
    position: BaseMembershipHistoryPosition,
}

impl HistorySynchronizer {
    pub(super) fn new(
        owner: Arc<MembershipOwner>,
        transport: Arc<dyn MembershipHistoryExchangePort>,
        address_refresh: Arc<dyn RefreshVerifiedPeerAddressPort>,
    ) -> Self {
        Self {
            evidence: ReconcileMembershipEvidenceUseCase::new(Arc::clone(&owner)),
            owner,
            transport,
            address_refresh,
            peer_locks: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// 同步已到期的对端。选定结果先落盘（游标与待同步标记），再发起网络交换。
    pub(super) async fn synchronize(
        &self,
        peers: Vec<DeviceId>,
    ) -> Result<HistorySyncReport, MembershipLedgerError> {
        let observation = MembershipRecoveryObservation::begin();
        let result = observation.scope(self.synchronize_inner(peers)).await;
        observation.finish(match &result {
            Ok(report) if report.stable_failure_count > 0 => MembershipRecoveryOutcome::Failed,
            Ok(report) if report.deferred_peer_count > 0 && report.completed_peer_count > 0 => {
                MembershipRecoveryOutcome::Partial
            }
            Ok(report) if report.deferred_peer_count > 0 => MembershipRecoveryOutcome::Deferred,
            Ok(report) if report.completed_peer_count > 0 => MembershipRecoveryOutcome::Completed,
            Ok(_) => MembershipRecoveryOutcome::NoWork,
            Err(
                MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired,
            ) => MembershipRecoveryOutcome::Corrupt,
            Err(_) => MembershipRecoveryOutcome::Deferred,
        });
        result
    }

    async fn synchronize_inner(
        &self,
        peers: Vec<DeviceId>,
    ) -> Result<HistorySyncReport, MembershipLedgerError> {
        let mut report = HistorySyncReport::default();
        if peers.is_empty() {
            return Ok(report);
        }
        let selected = self
            .owner
            .commit(|draft| {
                draft
                    .apply(LedgerInput::HistorySyncSelected {
                        peers: peers.clone(),
                    })
                    .map_err(ledger_error)
            })
            .await?;
        let space = selected.view.require_space()?;
        if space.ledger().local_status() != LedgerMemberStatus::Active {
            return Ok(report);
        }
        let history = space.history().clone();
        let sender = history
            .admission_facts_for(space.local_member())
            .cloned()
            .ok_or_else(MembershipLedgerError::corrupt)?;
        let position = history
            .current_position()
            .map_err(MembershipLedgerError::corrupt_from)?;
        let proofs: Vec<(DeviceId, HistoryProofRequirement)> = peers
            .into_iter()
            .map(|peer| {
                let proof = match space.ledger().peer(&peer) {
                    Some(PeerLink::Member(link)) if link.relation() == PeerRelation::Invalid => {
                        HistoryProofRequirement::Complete
                    }
                    _ => HistoryProofRequirement::Incremental,
                };
                (peer, proof)
            })
            .collect();
        let context = Arc::new(SyncContext {
            history,
            sender,
            position,
        });
        let deadline = tokio::time::Instant::now() + TOTAL_SYNC_BUDGET;
        // 网络交换并发有界；账本结果仍由下方顺序交回 Owner。
        let attempts = stream::iter(proofs.into_iter().map(|(peer, proof)| {
            let context = Arc::clone(&context);
            async move {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let result = if remaining.is_zero() {
                    Ok(PeerExchange::Finished(PeerSyncResult::Deferred))
                } else {
                    tokio::time::timeout(remaining, self.synchronize_peer(&peer, &context, proof))
                        .await
                        .unwrap_or(Ok(PeerExchange::Finished(PeerSyncResult::Deferred)))
                };
                (peer, result)
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_PEERS)
        .collect::<Vec<_>>()
        .await;
        for (peer, result) in attempts {
            match result? {
                PeerExchange::Finished(result) => {
                    match result {
                        PeerSyncResult::Confirmed | PeerSyncResult::AwaitingPeerDecision => {
                            report.completed_peer_count += 1
                        }
                        PeerSyncResult::Deferred => report.deferred_peer_count += 1,
                        PeerSyncResult::Diverged
                        | PeerSyncResult::Invalid
                        | PeerSyncResult::Rejected => report.stable_failure_count += 1,
                    }
                    self.owner
                        .commit(|draft| {
                            draft
                                .apply(LedgerInput::HistorySyncFinished {
                                    peer,
                                    synced_position: context.position.clone(),
                                    result,
                                })
                                .map_err(ledger_error)
                        })
                        .await?;
                }
                PeerExchange::DivergenceRecorded => report.stable_failure_count += 1,
            }
        }
        tracing::debug!(
            completed_peer_count = report.completed_peer_count,
            deferred_peer_count = report.deferred_peer_count,
            stable_failure_count = report.stable_failure_count,
            "成员历史同步轮次结束"
        );
        Ok(report)
    }

    async fn synchronize_peer(
        &self,
        peer: &DeviceId,
        context: &SyncContext,
        proof: HistoryProofRequirement,
    ) -> Result<PeerExchange, MembershipLedgerError> {
        let peer_lock = {
            let mut locks = self.peer_locks.lock().await;
            Arc::clone(
                locks
                    .entry(*peer)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _guard = peer_lock.lock().await;
        let result = match proof {
            HistoryProofRequirement::Complete => {
                self.exchange_complete_evidence(peer, context).await
            }
            HistoryProofRequirement::Incremental => self.exchange_summary(peer, context).await,
        };
        let exchange = match result {
            Ok(exchange) => exchange,
            Err(ExchangeFailure::Ledger(error)) => return Err(error),
            Err(ExchangeFailure::Transport(error)) => PeerExchange::Finished(match error {
                MembershipHistoryExchangeError::Offline { .. }
                | MembershipHistoryExchangeError::PairingInProgress
                | MembershipHistoryExchangeError::Transport { .. } => PeerSyncResult::Deferred,
                MembershipHistoryExchangeError::Rejected => PeerSyncResult::Rejected,
            }),
            // 对端回复不符合协议：保留同步欠账并按退避重试，不把一次异常回复当作稳定结论。
            Err(ExchangeFailure::Unexpected) => PeerExchange::Finished(PeerSyncResult::Deferred),
        };
        if matches!(exchange, PeerExchange::Finished(PeerSyncResult::Confirmed)) {
            self.address_refresh
                .refresh_verified_peer_address(peer)
                .await;
        }
        Ok(exchange)
    }

    async fn exchange_summary(
        &self,
        peer: &DeviceId,
        context: &SyncContext,
    ) -> Result<PeerExchange, ExchangeFailure> {
        let summary_transfer_id = context.position.history_digest;
        let reply = self
            .transport
            .exchange_membership_history(
                peer,
                MembershipHistoryMessage::SummaryV3(MembershipHistorySummaryV3 {
                    lineage_id: context.history.lineage_id().to_owned(),
                    current_position: context.position.clone(),
                    transfer_id: summary_transfer_id,
                    sender_admission: context.sender.clone(),
                }),
            )
            .await
            .map_err(ExchangeFailure::Transport)?;
        tracing::debug!(
            reply_kind = membership_message_kind(&reply),
            "成员历史摘要收到回复"
        );
        match reply {
            MembershipHistoryMessage::AckV3(ack)
                if ack_confirms_membership_history_target(
                    summary_transfer_id,
                    &context.position,
                    &ack,
                ) =>
            {
                Ok(PeerExchange::Finished(PeerSyncResult::Confirmed))
            }
            MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Confirmed {
                transfer_id,
                confirmed_position,
            }) if transfer_id == summary_transfer_id
                && confirms_an_ancestor(context, &confirmed_position) =>
            {
                Ok(PeerExchange::Finished(PeerSyncResult::AwaitingPeerDecision))
            }
            MembershipHistoryMessage::RequestSuffixV3(MembershipHistorySuffixRequestV3 {
                transfer_id: requested_transfer,
                known_position,
            }) if requested_transfer == summary_transfer_id => {
                let pages = context
                    .history
                    .export_suffix_pages_v4(context.sender.clone(), known_position)
                    // 导出失败只决定本轮交换延期（PeerSyncResult::Deferred），没有向上传递的调用方。
                    .map_err(|_| ExchangeFailure::Unexpected)?;
                tracing::debug!(page_count = pages.len(), "成员历史后缀已导出");
                self.send_suffix_pages(peer, pages, context).await
            }
            MembershipHistoryMessage::RequestConflictEvidenceV3(request)
                if request.transfer_id == summary_transfer_id =>
            {
                self.exchange_complete_evidence(peer, context).await
            }
            _ => {
                tracing::debug!("成员历史摘要收到不匹配的回复");
                Err(ExchangeFailure::Unexpected)
            }
        }
    }

    async fn exchange_complete_evidence(
        &self,
        peer: &DeviceId,
        context: &SyncContext,
    ) -> Result<PeerExchange, ExchangeFailure> {
        let pages = context
            .history
            .export_conflict_evidence_pages_v2(context.sender.clone())
            // 导出失败只决定本轮交换延期（PeerSyncResult::Deferred），没有向上传递的调用方。
            .map_err(|_| ExchangeFailure::Unexpected)?;
        let reply = self
            .transport
            .exchange_membership_history(
                peer,
                MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
                    transfer_id: context.position.history_digest,
                    pages,
                }),
            )
            .await
            .map_err(ExchangeFailure::Transport)?;
        let MembershipHistoryMessage::ConflictEvidenceV3(evidence) = reply else {
            return Ok(PeerExchange::Finished(PeerSyncResult::Deferred));
        };
        let Some(verified) = self
            .evidence
            .execute(peer, &evidence)
            .await
            .map_err(ExchangeFailure::Ledger)?
        else {
            return Ok(PeerExchange::Finished(PeerSyncResult::Deferred));
        };
        Ok(match verified.relationship {
            MembershipHistoryRelationship::Consistent => {
                PeerExchange::Finished(PeerSyncResult::Confirmed)
            }
            MembershipHistoryRelationship::Diverged => {
                describe_membership_conflict();
                PeerExchange::DivergenceRecorded
            }
            _ => PeerExchange::Finished(PeerSyncResult::Deferred),
        })
    }

    async fn send_suffix_pages(
        &self,
        peer: &DeviceId,
        pages: Vec<MembershipHistorySuffixPageV4>,
        context: &SyncContext,
    ) -> Result<PeerExchange, ExchangeFailure> {
        let transfer_id = pages
            .first()
            .map(|page| page.transfer_id())
            .ok_or(ExchangeFailure::Unexpected)?;
        let mut next_page_index = 0u32;
        for _ in 0..=pages.len() {
            let page = pages
                .get(next_page_index as usize)
                .cloned()
                .ok_or(ExchangeFailure::Unexpected)?;
            let reply = self
                .transport
                .exchange_membership_history(peer, MembershipHistoryMessage::SuffixPageV4(page))
                .await
                .map_err(ExchangeFailure::Transport)?;
            let MembershipHistoryMessage::AckV3(ack) = reply else {
                tracing::debug!("成员历史后缀页收到非 ACK 回复");
                return Err(ExchangeFailure::Unexpected);
            };
            tracing::debug!(
                page_number = next_page_index.saturating_add(1),
                page_count = pages.len(),
                ack_kind = membership_ack_kind(&ack),
                "成员历史后缀页收到 ACK"
            );
            match ack {
                MembershipHistoryAckV3::NeedsEvidence => {
                    return self.exchange_complete_evidence(peer, context).await;
                }
                MembershipHistoryAckV3::Continue {
                    transfer_id: acknowledged_transfer,
                    next_page_index: requested_page,
                } if acknowledged_transfer == transfer_id
                    && requested_page == next_page_index.saturating_add(1)
                    && (requested_page as usize) < pages.len() =>
                {
                    next_page_index = requested_page;
                }
                MembershipHistoryAckV3::Confirmed {
                    transfer_id: acknowledged_transfer,
                    confirmed_position,
                } if next_page_index as usize + 1 == pages.len()
                    && acknowledged_transfer == transfer_id
                    && confirmed_position == context.position =>
                {
                    return Ok(PeerExchange::Finished(PeerSyncResult::Confirmed));
                }
                MembershipHistoryAckV3::Confirmed {
                    transfer_id: acknowledged_transfer,
                    confirmed_position,
                } if next_page_index as usize + 1 == pages.len()
                    && acknowledged_transfer == transfer_id
                    && confirms_an_ancestor(context, &confirmed_position) =>
                {
                    return Ok(PeerExchange::Finished(PeerSyncResult::AwaitingPeerDecision));
                }
                MembershipHistoryAckV3::Diverged => {
                    describe_membership_conflict();
                    return Ok(PeerExchange::Finished(PeerSyncResult::Diverged));
                }
                MembershipHistoryAckV3::Invalid => {
                    return Ok(PeerExchange::Finished(PeerSyncResult::Invalid));
                }
                MembershipHistoryAckV3::Continue { .. }
                | MembershipHistoryAckV3::Confirmed { .. }
                | MembershipHistoryAckV3::RestrictedApplied
                | MembershipHistoryAckV3::RestrictedConsistent => {
                    return Err(ExchangeFailure::Unexpected);
                }
            }
        }
        Err(ExchangeFailure::Unexpected)
    }
}

/// 对端确认的是本机当前位置的严格祖先：对端停在本机发起、尚待其决定的一项移除之前。是否确实如此由
/// 成员账本核实。
fn confirms_an_ancestor(context: &SyncContext, confirmed: &BaseMembershipHistoryPosition) -> bool {
    context.history.contains_strict_ancestor_position(confirmed)
}

enum ExchangeFailure {
    Transport(MembershipHistoryExchangeError),
    Ledger(MembershipLedgerError),
    Unexpected,
}

fn membership_message_kind(message: &MembershipHistoryMessage) -> &'static str {
    match message {
        MembershipHistoryMessage::SummaryV3(_) => "summary",
        MembershipHistoryMessage::RequestSuffixV3(_) => "request_suffix",
        MembershipHistoryMessage::SuffixPageV4(_) => "suffix_page",
        MembershipHistoryMessage::RequestConflictEvidenceV3(_) => "request_conflict_evidence",
        MembershipHistoryMessage::ConflictEvidenceV3(_) => "conflict_evidence",
        MembershipHistoryMessage::AckV3(ack) => membership_ack_kind(ack),
        MembershipHistoryMessage::RestrictedEventV3(_) => "restricted_event",
        MembershipHistoryMessage::RestrictedDecisionV3(_) => "restricted_decision",
    }
}

fn membership_ack_kind(ack: &MembershipHistoryAckV3) -> &'static str {
    match ack {
        MembershipHistoryAckV3::Continue { .. } => "ack_continue",
        MembershipHistoryAckV3::Confirmed { .. } => "ack_confirmed",
        MembershipHistoryAckV3::RestrictedApplied => "ack_restricted_applied",
        MembershipHistoryAckV3::RestrictedConsistent => "ack_restricted_consistent",
        MembershipHistoryAckV3::Diverged => "ack_diverged",
        MembershipHistoryAckV3::Invalid => "ack_invalid",
        MembershipHistoryAckV3::NeedsEvidence => "ack_needs_evidence",
    }
}
