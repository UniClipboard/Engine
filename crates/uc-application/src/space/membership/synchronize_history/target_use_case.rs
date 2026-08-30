use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::{stream, StreamExt};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    ack_confirms_membership_history_target, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, MembershipHistorySuffixRequestV3,
    MembershipHistorySummaryV3,
};
use uc_core::ports::ClockPort;

use crate::space::membership::{
    CurrentSpaceMemberScopePort, MembershipLedger, MembershipLedgerError, PeerReconciliationRecord,
    SpaceMemberPauseReason,
};
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
    SynchronizeMembershipMaintenancePort,
};

use super::{MembershipSyncReport, MembershipSyncTarget, SynchronizeMembershipHistoryError};

const TOTAL_SYNC_BUDGET: Duration = Duration::from_secs(10);
const MAX_PEERS_PER_ROUND: usize = 8;
const MAX_CONCURRENT_PEERS: usize = 4;
const INITIAL_RETRY_DELAY_MS: i64 = 1_000;
const MAX_RETRY_DELAY_MS: i64 = 5 * 60 * 1_000;

pub(crate) struct SynchronizeMembershipHistoryUseCase {
    ledger: Arc<MembershipLedger>,
    current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    transport: Arc<dyn MembershipHistoryExchangePort>,
    clock: Arc<dyn ClockPort>,
    execution_lock: tokio::sync::Mutex<()>,
    peer_locks: tokio::sync::Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
    ledger_commit_lock: tokio::sync::Mutex<()>,
}

impl SynchronizeMembershipHistoryUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
        transport: Arc<dyn MembershipHistoryExchangePort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            current_scope,
            transport,
            clock,
            execution_lock: tokio::sync::Mutex::new(()),
            peer_locks: tokio::sync::Mutex::new(BTreeMap::new()),
            ledger_commit_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        target: MembershipSyncTarget,
    ) -> Result<MembershipSyncReport, SynchronizeMembershipHistoryError> {
        match target {
            MembershipSyncTarget::AllCurrentPeers => {
                let _guard = self.execution_lock.lock().await;
                let peers = self.current_sync_peers().await?;
                let peers = self.select_due_peers(peers).await?;
                self.execute_peers(peers, Some(TOTAL_SYNC_BUDGET)).await
            }
            MembershipSyncTarget::AuthenticatedPeer(peer) => {
                if !self.current_sync_peers().await?.contains(&peer) {
                    return Err(SynchronizeMembershipHistoryError::CurrentScopeUnavailable);
                }
                self.execute_peers(vec![peer], None).await
            }
        }
    }

    pub(super) async fn select_due_peers(
        &self,
        mut eligible: Vec<DeviceId>,
    ) -> Result<Vec<DeviceId>, SynchronizeMembershipHistoryError> {
        let now_ms = self.clock.now_ms();
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let current_position = snapshot
            .history()
            .ok_or(SynchronizeMembershipHistoryError::RecoveryRequired)?
            .current_position()
            .map_err(|_| SynchronizeMembershipHistoryError::RecoveryRequired)?;
        let eligible_peer_count = eligible.len();
        let mut already_confirmed_count = 0usize;
        let mut retry_delayed_count = 0usize;
        let mut isolated_count = 0usize;
        eligible.retain(|peer| {
            let Some(record) = snapshot.record().peer_reconciliation.get(peer) else {
                return true;
            };
            if record.confirmed_position.as_ref() == Some(&current_position) {
                already_confirmed_count += 1;
                return false;
            }
            if record.sync_state.next_attempt_at_ms > now_ms {
                retry_delayed_count += 1;
                return false;
            }
            if matches!(
                record.relationship,
                uc_core::membership::MembershipHistoryRelationship::Diverged
                    | uc_core::membership::MembershipHistoryRelationship::Invalid
            ) {
                isolated_count += 1;
                return false;
            }
            true
        });
        eligible.sort();
        eligible.dedup();
        if let Some(cursor) = snapshot.record().history_sync_cursor.as_ref() {
            let split = eligible.partition_point(|peer| peer <= cursor);
            eligible.rotate_left(split);
        }
        eligible.truncate(MAX_PEERS_PER_ROUND);
        tracing::debug!(
            eligible_peer_count,
            selected_peer_count = eligible.len(),
            already_confirmed_count,
            retry_delayed_count,
            isolated_count,
            "成员历史反熵已选择到期任务"
        );
        let Some(last_selected) = eligible.last().cloned() else {
            return Ok(eligible);
        };
        let selected = eligible.clone();
        self.ledger
            .compare_and_commit(|record| {
                let pending_revision = record.revision.saturating_add(1);
                for peer in &selected {
                    let peer_record = record
                        .peer_reconciliation
                        .entry(peer.clone())
                        .or_insert_with(|| PeerReconciliationRecord {
                            peer_device_id: peer.clone(),
                            relationship:
                                uc_core::membership::MembershipHistoryRelationship::Unknown,
                            confirmed_position: None,
                            sync_state: Default::default(),
                            restricted_delivery: Vec::new(),
                            updated_at_ms: 0,
                        });
                    peer_record
                        .sync_state
                        .pending_since_revision
                        .get_or_insert(pending_revision);
                }
                // 游标在网络调用前落盘；即使进程中断，下轮也会从后续 peer 开始。
                record.history_sync_cursor = Some(last_selected.clone());
                Ok(())
            })
            .await
            .map_err(map_ledger_error)?;
        Ok(eligible)
    }

    async fn current_sync_peers(&self) -> Result<Vec<DeviceId>, SynchronizeMembershipHistoryError> {
        let scope = self
            .current_scope
            .snapshot()
            .await
            .map_err(|_| SynchronizeMembershipHistoryError::CurrentScopeUnavailable)?;
        if !scope.local_member_active {
            return Err(SynchronizeMembershipHistoryError::RecoveryRequired);
        }
        let mut peers = scope.usable_peer_device_ids;
        peers.extend(
            scope
                .paused_peer_devices
                .into_iter()
                .filter(|peer| {
                    matches!(
                        peer.reason,
                        SpaceMemberPauseReason::RelationshipUnconfirmed
                            | SpaceMemberPauseReason::PendingLocalDecision
                            | SpaceMemberPauseReason::UpgradeRequired
                    )
                })
                .map(|peer| peer.device_id),
        );
        peers.sort();
        peers.dedup();
        Ok(peers)
    }

    async fn execute_peers(
        &self,
        peers: Vec<DeviceId>,
        budget: Option<Duration>,
    ) -> Result<MembershipSyncReport, SynchronizeMembershipHistoryError> {
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let history = snapshot
            .history()
            .ok_or(SynchronizeMembershipHistoryError::RecoveryRequired)?;
        let local_member = snapshot
            .record()
            .local_member_instance
            .ok_or(SynchronizeMembershipHistoryError::RecoveryRequired)?;
        if !snapshot.record().local_join_active || !history.active_members().contains(&local_member)
        {
            return Err(SynchronizeMembershipHistoryError::RecoveryRequired);
        }
        let sender = history
            .admission_facts_for(local_member)
            .cloned()
            .ok_or(SynchronizeMembershipHistoryError::RecoveryRequired)?;
        let history = Arc::new(history.clone());
        let sender = Arc::new(sender);
        let position = history
            .current_position()
            .map_err(|_| SynchronizeMembershipHistoryError::RecoveryRequired)?;
        let lineage_id = Arc::new(history.lineage_id().to_owned());
        let deadline = budget.map(|budget| tokio::time::Instant::now() + budget);
        let mut report = MembershipSyncReport::default();
        // 网络交换并发有界；账本结果仍由下方顺序提交，避免把冲突重试泄漏给 transport。
        let attempts = stream::iter(peers.into_iter().map(|peer| {
            let history = Arc::clone(&history);
            let sender = Arc::clone(&sender);
            let position = position.clone();
            let lineage_id = Arc::clone(&lineage_id);
            async move {
                let result = match deadline {
                    Some(deadline) => {
                        let remaining =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remaining.is_zero() {
                            Err(PeerSyncError::Deferred)
                        } else {
                            tokio::time::timeout(
                                remaining,
                                self.synchronize_peer(&peer, history, sender, position, lineage_id),
                            )
                            .await
                            .unwrap_or(Err(PeerSyncError::Deferred))
                        }
                    }
                    None => {
                        self.synchronize_peer(&peer, history, sender, position, lineage_id)
                            .await
                    }
                };
                (peer, result)
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_PEERS)
        .collect::<Vec<_>>()
        .await;
        for (peer, result) in attempts {
            match result {
                Ok(()) => report.completed_peer_count += 1,
                Err(PeerSyncError::Deferred) => {
                    self.commit_deferred_attempt(&peer).await?;
                    report.deferred_peer_count += 1;
                }
                Err(PeerSyncError::Stable) => report.stable_failure_count += 1,
            }
        }
        tracing::debug!(
            completed_peer_count = report.completed_peer_count,
            deferred_peer_count = report.deferred_peer_count,
            stable_failure_count = report.stable_failure_count,
            "成员历史反熵轮次结束"
        );
        Ok(report)
    }

    async fn commit_deferred_attempt(
        &self,
        peer: &DeviceId,
    ) -> Result<(), SynchronizeMembershipHistoryError> {
        let _guard = self.ledger_commit_lock.lock().await;
        let peer = peer.clone();
        let now_ms = self.clock.now_ms();
        self.ledger
            .compare_and_commit(|record| {
                let peer_record = record
                    .peer_reconciliation
                    .get_mut(&peer)
                    .ok_or(MembershipLedgerError::RecoveryRequired)?;
                peer_record.sync_state.retry_attempt = peer_record
                    .sync_state
                    .retry_attempt
                    .checked_add(1)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                let shift = peer_record
                    .sync_state
                    .retry_attempt
                    .saturating_sub(1)
                    .min(18);
                let delay = INITIAL_RETRY_DELAY_MS
                    .checked_shl(shift)
                    .unwrap_or(MAX_RETRY_DELAY_MS)
                    .min(MAX_RETRY_DELAY_MS);
                peer_record.sync_state.next_attempt_at_ms = now_ms.saturating_add(delay);
                peer_record.sync_state.last_attempt_outcome =
                    crate::space::membership::PeerHistorySyncOutcome::Deferred;
                Ok(())
            })
            .await
            .map(|_| ())
            .map_err(map_ledger_error)
    }

    async fn synchronize_peer(
        &self,
        peer: &DeviceId,
        history: Arc<uc_core::membership::VersionedMembershipHistory>,
        sender: Arc<uc_core::membership::AdmissionChangeFacts>,
        position: uc_core::membership::BaseMembershipHistoryPosition,
        lineage_id: Arc<String>,
    ) -> Result<(), PeerSyncError> {
        let peer_lock = {
            let mut locks = self.peer_locks.lock().await;
            Arc::clone(
                locks
                    .entry(peer.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _guard = peer_lock.lock().await;
        let summary_transfer_id = position.history_digest;
        let reply = self
            .transport
            .exchange_membership_history(
                peer,
                MembershipHistoryMessage::SummaryV3(MembershipHistorySummaryV3 {
                    lineage_id: (*lineage_id).clone(),
                    current_position: position.clone(),
                    transfer_id: summary_transfer_id,
                    sender_admission: (*sender).clone(),
                }),
            )
            .await
            .map_err(map_exchange_error)?;
        tracing::debug!(
            reply_kind = membership_message_kind(&reply),
            "成员历史摘要收到回复"
        );
        match reply {
            MembershipHistoryMessage::AckV3(ack)
                if ack_confirms_membership_history_target(summary_transfer_id, &position, &ack) =>
            {
                self.commit_peer_relationship(
                    peer,
                    uc_core::membership::MembershipHistoryRelationship::Consistent,
                    Some(position),
                )
                .await?;
                return Ok(());
            }
            MembershipHistoryMessage::RequestSuffixV3(MembershipHistorySuffixRequestV3 {
                transfer_id: requested_transfer,
                known_position,
            }) if requested_transfer == summary_transfer_id => {
                let pages = history
                    .export_suffix_pages_v3((*sender).clone(), known_position)
                    .map_err(|_| PeerSyncError::Stable)?;
                tracing::debug!(page_count = pages.len(), "成员历史后缀已导出");
                return self.send_suffix_pages(peer, pages, position).await;
            }
            _ => {
                tracing::debug!("成员历史摘要收到不匹配的稳定回复");
                return Err(PeerSyncError::Stable);
            }
        }
    }

    async fn send_suffix_pages(
        &self,
        peer: &DeviceId,
        pages: Vec<uc_core::membership::MembershipHistorySuffixPageV3>,
        position: uc_core::membership::BaseMembershipHistoryPosition,
    ) -> Result<(), PeerSyncError> {
        let transfer_id = pages
            .first()
            .map(|page| page.transfer_id())
            .ok_or(PeerSyncError::Stable)?;
        let mut next_page_index = 0u32;
        for _ in 0..=pages.len() {
            let page = pages
                .get(next_page_index as usize)
                .cloned()
                .ok_or(PeerSyncError::Stable)?;
            let reply = self
                .transport
                .exchange_membership_history(peer, MembershipHistoryMessage::SuffixPageV3(page))
                .await
                .map_err(map_exchange_error)?;
            let MembershipHistoryMessage::AckV3(ack) = reply else {
                tracing::debug!("成员历史后缀页收到非 ACK 回复");
                return Err(PeerSyncError::Stable);
            };
            tracing::debug!(
                page_number = next_page_index.saturating_add(1),
                page_count = pages.len(),
                ack_kind = membership_ack_kind(&ack),
                "成员历史后缀页收到 ACK"
            );
            match ack {
                uc_core::membership::MembershipHistoryAckV3::Continue {
                    transfer_id: acknowledged_transfer,
                    next_page_index: requested_page,
                } if acknowledged_transfer == transfer_id
                    && requested_page == next_page_index.saturating_add(1)
                    && (requested_page as usize) < pages.len() =>
                {
                    next_page_index = requested_page;
                }
                uc_core::membership::MembershipHistoryAckV3::Confirmed {
                    transfer_id: acknowledged_transfer,
                    confirmed_position,
                } if next_page_index as usize + 1 == pages.len()
                    && acknowledged_transfer == transfer_id
                    && confirmed_position == position =>
                {
                    self.commit_peer_relationship(
                        peer,
                        uc_core::membership::MembershipHistoryRelationship::Consistent,
                        Some(position),
                    )
                    .await?;
                    return Ok(());
                }
                uc_core::membership::MembershipHistoryAckV3::Diverged => {
                    self.commit_peer_relationship(
                        peer,
                        uc_core::membership::MembershipHistoryRelationship::Diverged,
                        None,
                    )
                    .await?;
                    return Err(PeerSyncError::Stable);
                }
                uc_core::membership::MembershipHistoryAckV3::Invalid => {
                    self.commit_peer_relationship(
                        peer,
                        uc_core::membership::MembershipHistoryRelationship::Invalid,
                        None,
                    )
                    .await?;
                    return Err(PeerSyncError::Stable);
                }
                uc_core::membership::MembershipHistoryAckV3::Continue { .. }
                | uc_core::membership::MembershipHistoryAckV3::Confirmed { .. }
                | uc_core::membership::MembershipHistoryAckV3::RestrictedApplied
                | uc_core::membership::MembershipHistoryAckV3::RestrictedConsistent => {
                    return Err(PeerSyncError::Stable);
                }
            }
        }
        Err(PeerSyncError::Stable)
    }

    async fn commit_peer_relationship(
        &self,
        peer: &DeviceId,
        relationship: uc_core::membership::MembershipHistoryRelationship,
        confirmed_position: Option<uc_core::membership::BaseMembershipHistoryPosition>,
    ) -> Result<(), PeerSyncError> {
        let _guard = self.ledger_commit_lock.lock().await;
        let peer = peer.clone();
        self.ledger
            .compare_and_commit(|record| {
                record
                    .peer_reconciliation
                    .entry(peer.clone())
                    .and_modify(|current| {
                        current.relationship = relationship;
                        current.confirmed_position = confirmed_position.clone();
                        current.sync_state.retry_attempt = 0;
                        current.sync_state.next_attempt_at_ms = 0;
                        current.sync_state.pending_since_revision = None;
                        current.sync_state.last_attempt_outcome = if confirmed_position.is_some() {
                            crate::space::membership::PeerHistorySyncOutcome::Acked
                        } else {
                            crate::space::membership::PeerHistorySyncOutcome::StableRejected
                        };
                    })
                    .or_insert(PeerReconciliationRecord {
                        peer_device_id: peer,
                        relationship,
                        confirmed_position,
                        sync_state: Default::default(),
                        restricted_delivery: Vec::new(),
                        updated_at_ms: 0,
                    });
                Ok(())
            })
            .await
            .map(|_| ())
            .map_err(|error| match error {
                MembershipLedgerError::Conflict
                | MembershipLedgerError::Locked
                | MembershipLedgerError::Unavailable => PeerSyncError::Deferred,
                MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
                    PeerSyncError::Stable
                }
            })
    }
}

fn membership_message_kind(message: &MembershipHistoryMessage) -> &'static str {
    match message {
        MembershipHistoryMessage::SummaryV3(_) => "summary",
        MembershipHistoryMessage::RequestSuffixV3(_) => "request_suffix",
        MembershipHistoryMessage::SuffixPageV3(_) => "suffix_page",
        MembershipHistoryMessage::AckV3(ack) => membership_ack_kind(ack),
        MembershipHistoryMessage::RestrictedEventV3(_) => "restricted_event",
        MembershipHistoryMessage::RestrictedDecisionV3(_) => "restricted_decision",
    }
}

fn membership_ack_kind(ack: &uc_core::membership::MembershipHistoryAckV3) -> &'static str {
    match ack {
        uc_core::membership::MembershipHistoryAckV3::Continue { .. } => "ack_continue",
        uc_core::membership::MembershipHistoryAckV3::Confirmed { .. } => "ack_confirmed",
        uc_core::membership::MembershipHistoryAckV3::RestrictedApplied => "ack_restricted_applied",
        uc_core::membership::MembershipHistoryAckV3::RestrictedConsistent => {
            "ack_restricted_consistent"
        }
        uc_core::membership::MembershipHistoryAckV3::Diverged => "ack_diverged",
        uc_core::membership::MembershipHistoryAckV3::Invalid => "ack_invalid",
    }
}

enum PeerSyncError {
    Deferred,
    Stable,
}

fn map_exchange_error(error: MembershipHistoryExchangeError) -> PeerSyncError {
    match error {
        MembershipHistoryExchangeError::Offline | MembershipHistoryExchangeError::Transport => {
            PeerSyncError::Deferred
        }
        MembershipHistoryExchangeError::Rejected => PeerSyncError::Stable,
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> SynchronizeMembershipHistoryError {
    match error {
        MembershipLedgerError::Locked
        | MembershipLedgerError::Conflict
        | MembershipLedgerError::Unavailable => SynchronizeMembershipHistoryError::Unavailable,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            SynchronizeMembershipHistoryError::RecoveryRequired
        }
    }
}

#[async_trait::async_trait]
impl SynchronizeMembershipMaintenancePort for SynchronizeMembershipHistoryUseCase {
    async fn periodic_synchronization_required(
        &self,
    ) -> Result<bool, MembershipMaintenanceStepOutcome> {
        let scope = match self.current_scope.snapshot().await {
            Ok(scope) => scope,
            Err(crate::space::membership::CurrentSpaceMemberScopeError::NoCurrentSpace) => {
                return Ok(false);
            }
            Err(
                crate::space::membership::CurrentSpaceMemberScopeError::Locked
                | crate::space::membership::CurrentSpaceMemberScopeError::Unavailable,
            ) => return Err(MembershipMaintenanceStepOutcome::Deferred),
            Err(crate::space::membership::CurrentSpaceMemberScopeError::RecoveryRequired) => {
                return Err(MembershipMaintenanceStepOutcome::Corrupt);
            }
        };
        if !scope.local_member_active {
            return Err(MembershipMaintenanceStepOutcome::Corrupt);
        }
        let mut eligible_peers = scope.usable_peer_device_ids;
        let has_relationship_work = scope.paused_peer_devices.into_iter().any(|peer| {
            matches!(
                peer.reason,
                SpaceMemberPauseReason::RelationshipUnconfirmed
                    | SpaceMemberPauseReason::PendingLocalDecision
                    | SpaceMemberPauseReason::UpgradeRequired
            )
        });
        if has_relationship_work {
            return Ok(true);
        }
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(|error| match error {
                MembershipLedgerError::Locked
                | MembershipLedgerError::Conflict
                | MembershipLedgerError::Unavailable => MembershipMaintenanceStepOutcome::Deferred,
                MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
                    MembershipMaintenanceStepOutcome::Corrupt
                }
            })?;
        let current_position = snapshot
            .history()
            .ok_or(MembershipMaintenanceStepOutcome::Corrupt)?
            .current_position()
            .map_err(|_| MembershipMaintenanceStepOutcome::Corrupt)?;
        eligible_peers.sort();
        eligible_peers.dedup();

        // `Consistent` 只表示历史没有分叉，不能证明对端已经收到本机最新位置。
        // 周期维护必须以认证 ACK 保存的逐 peer 水位作为最终重试依据。
        Ok(eligible_peers.into_iter().any(|peer| {
            snapshot
                .record()
                .peer_reconciliation
                .get(&peer)
                .and_then(|record| record.confirmed_position.as_ref())
                != Some(&current_position)
        }))
    }

    async fn synchronize_membership(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        let target = match trigger {
            MembershipMaintenanceTrigger::PeerOnline(peer) => {
                MembershipSyncTarget::AuthenticatedPeer(peer.clone())
            }
            MembershipMaintenanceTrigger::Startup
            | MembershipMaintenanceTrigger::Resume
            | MembershipMaintenanceTrigger::Periodic
            | MembershipMaintenanceTrigger::StateChanged => MembershipSyncTarget::AllCurrentPeers,
        };
        match self.execute(target).await {
            Ok(report) if report.stable_failure_count > 0 => {
                MembershipMaintenanceStepOutcome::StableFailure
            }
            Ok(report) if report.deferred_peer_count > 0 => {
                MembershipMaintenanceStepOutcome::Deferred
            }
            Ok(_) => MembershipMaintenanceStepOutcome::Completed,
            Err(SynchronizeMembershipHistoryError::RecoveryRequired) => {
                MembershipMaintenanceStepOutcome::Corrupt
            }
            Err(SynchronizeMembershipHistoryError::CurrentScopeUnavailable)
            | Err(SynchronizeMembershipHistoryError::Unavailable) => {
                MembershipMaintenanceStepOutcome::Deferred
            }
        }
    }
}
