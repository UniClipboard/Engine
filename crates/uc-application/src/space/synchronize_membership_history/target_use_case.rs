use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    MembershipHistoryV2Ack,
};

use crate::space::maintain_space_membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
    SynchronizeMembershipMaintenancePort,
};
use crate::space::membership_ledger::{
    CurrentSpaceMemberScopePort, MembershipLedger, MembershipLedgerError, PeerReconciliationRecord,
    SpaceMemberPauseReason,
};

use super::{MembershipSyncReport, MembershipSyncTarget, SynchronizeMembershipHistoryError};

const TOTAL_SYNC_BUDGET: Duration = Duration::from_secs(10);

pub(crate) struct SynchronizeMembershipHistoryUseCase {
    ledger: Arc<MembershipLedger>,
    current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    transport: Arc<dyn MembershipHistoryExchangePort>,
    execution_lock: tokio::sync::Mutex<()>,
    peer_locks: tokio::sync::Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
}

impl SynchronizeMembershipHistoryUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
        transport: Arc<dyn MembershipHistoryExchangePort>,
    ) -> Self {
        Self {
            ledger,
            current_scope,
            transport,
            execution_lock: tokio::sync::Mutex::new(()),
            peer_locks: tokio::sync::Mutex::new(BTreeMap::new()),
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
        let pages = Arc::new(
            history
                .export_reconciliation_pages_v2(sender)
                .map_err(|_| SynchronizeMembershipHistoryError::RecoveryRequired)?,
        );
        let position = history
            .current_position()
            .map_err(|_| SynchronizeMembershipHistoryError::RecoveryRequired)?;
        let deadline = budget.map(|budget| tokio::time::Instant::now() + budget);
        let mut report = MembershipSyncReport::default();
        for peer in peers {
            let result = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        report.deferred_peer_count += 1;
                        continue;
                    }
                    match tokio::time::timeout(
                        remaining,
                        self.synchronize_peer(&peer, Arc::clone(&pages), position.clone()),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(PeerSyncError::Deferred),
                    }
                }
                None => {
                    self.synchronize_peer(&peer, Arc::clone(&pages), position.clone())
                        .await
                }
            };
            match result {
                Ok(()) => report.completed_peer_count += 1,
                Err(PeerSyncError::Deferred) => report.deferred_peer_count += 1,
                Err(PeerSyncError::Stable) => report.stable_failure_count += 1,
            }
        }
        Ok(report)
    }

    async fn synchronize_peer(
        &self,
        peer: &DeviceId,
        pages: Arc<Vec<uc_core::membership::MembershipHistoryPageV2>>,
        position: uc_core::membership::BaseMembershipHistoryPosition,
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
                .exchange_membership_history(peer, MembershipHistoryMessage::HistoryPageV2(page))
                .await
                .map_err(|error| match error {
                    MembershipHistoryExchangeError::Offline
                    | MembershipHistoryExchangeError::Transport => PeerSyncError::Deferred,
                    MembershipHistoryExchangeError::Rejected => PeerSyncError::Stable,
                })?;
            let MembershipHistoryMessage::AckV2(ack) = reply else {
                return Err(PeerSyncError::Stable);
            };
            match ack {
                MembershipHistoryV2Ack::Continue {
                    transfer_id: acknowledged_transfer,
                    next_page_index: requested_page,
                } if acknowledged_transfer == transfer_id
                    && requested_page == next_page_index.saturating_add(1)
                    && (requested_page as usize) < pages.len() =>
                {
                    next_page_index = requested_page;
                }
                MembershipHistoryV2Ack::Consistent | MembershipHistoryV2Ack::UpdatesApplied
                    if next_page_index as usize + 1 == pages.len() =>
                {
                    self.commit_peer_relationship(
                        peer,
                        uc_core::membership::MembershipHistoryRelationship::Consistent,
                        Some(position),
                    )
                    .await?;
                    return Ok(());
                }
                MembershipHistoryV2Ack::Diverged => {
                    self.commit_peer_relationship(
                        peer,
                        uc_core::membership::MembershipHistoryRelationship::Diverged,
                        None,
                    )
                    .await?;
                    return Err(PeerSyncError::Stable);
                }
                MembershipHistoryV2Ack::Invalid => {
                    self.commit_peer_relationship(
                        peer,
                        uc_core::membership::MembershipHistoryRelationship::Invalid,
                        None,
                    )
                    .await?;
                    return Err(PeerSyncError::Stable);
                }
                MembershipHistoryV2Ack::Continue { .. }
                | MembershipHistoryV2Ack::Consistent
                | MembershipHistoryV2Ack::UpdatesApplied => {
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
        let peer = peer.clone();
        self.ledger
            .compare_and_commit(|record| {
                record
                    .peer_reconciliation
                    .entry(peer.clone())
                    .and_modify(|current| {
                        current.relationship = relationship;
                        current.confirmed_position = confirmed_position.clone();
                    })
                    .or_insert(PeerReconciliationRecord {
                        peer_device_id: peer,
                        relationship,
                        confirmed_position,
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

enum PeerSyncError {
    Deferred,
    Stable,
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
            Err(crate::space::membership_ledger::CurrentSpaceMemberScopeError::NoCurrentSpace) => {
                return Ok(false);
            }
            Err(
                crate::space::membership_ledger::CurrentSpaceMemberScopeError::Locked
                | crate::space::membership_ledger::CurrentSpaceMemberScopeError::Unavailable,
            ) => return Err(MembershipMaintenanceStepOutcome::Deferred),
            Err(
                crate::space::membership_ledger::CurrentSpaceMemberScopeError::RecoveryRequired,
            ) => {
                return Err(MembershipMaintenanceStepOutcome::Corrupt);
            }
        };
        if !scope.local_member_active {
            return Err(MembershipMaintenanceStepOutcome::Corrupt);
        }
        Ok(scope.paused_peer_devices.into_iter().any(|peer| {
            matches!(
                peer.reason,
                SpaceMemberPauseReason::RelationshipUnconfirmed
                    | SpaceMemberPauseReason::PendingLocalDecision
                    | SpaceMemberPauseReason::UpgradeRequired
            )
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
