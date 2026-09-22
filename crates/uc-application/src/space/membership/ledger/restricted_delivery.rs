use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::ports::ClockPort;

use crate::space::membership::{
    DeliverRestrictedMembershipPort, MembershipMaintenanceReport, MembershipMaintenanceStepOutcome,
};

use super::{
    MembershipLedger, MembershipLedgerError, PeerReconciliationRecord, RestrictedMembershipDelivery,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RestrictedMembershipDeliveryError {
    #[error("restricted membership delivery is deferred")]
    Deferred,
    #[error("restricted membership delivery was rejected")]
    Rejected,
}

#[async_trait]
pub trait RestrictedMembershipDeliveryPort: Send + Sync {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError>;
}

/// 已不在当前成员集合中的设备只在该窗口内接收受限投递。
///
/// 被移除的设备可能长期离线或拒绝连接；窗口结束后两端各自收尾，本机结束投递并
/// 移除该设备的核对记录，不让一次移除永远占用维护轮次和设备更新状态。
pub(super) const REMOVED_PEER_DELIVERY_WINDOW_MS: i64 = 300_000;

pub(crate) struct DeliverRestrictedMembershipUseCase {
    ledger: Arc<MembershipLedger>,
    delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
    clock: Arc<dyn ClockPort>,
}

enum RemovedPeerWindow {
    Open,
    Unstarted,
    Expired,
}

impl DeliverRestrictedMembershipUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            delivery,
            clock,
        }
    }

    pub(crate) async fn execute(&self) -> MembershipMaintenanceReport {
        let mut report = MembershipMaintenanceReport::default();
        let now_ms = self.clock.now_ms();
        let snapshot = match self.ledger.load_verified().await {
            Ok(snapshot) => snapshot,
            Err(_) => {
                report.corrupt_count = 1;
                return report;
            }
        };
        let mut plans = Vec::new();
        for (peer, record) in &snapshot.record().peer_reconciliation {
            if record.restricted_delivery.is_empty() {
                continue;
            }
            let removed = snapshot
                .history()
                .is_some_and(|history| history.effective_member_for_device(peer).is_none());
            let window = if !removed {
                RemovedPeerWindow::Open
            } else if record.updated_at_ms <= 0 {
                RemovedPeerWindow::Unstarted
            } else if now_ms.saturating_sub(record.updated_at_ms) >= REMOVED_PEER_DELIVERY_WINDOW_MS
            {
                RemovedPeerWindow::Expired
            } else {
                RemovedPeerWindow::Open
            };
            match window {
                RemovedPeerWindow::Expired => {
                    match self.ledger.end_removed_peer_delivery(record.clone()).await {
                        Ok(()) => report.completed_count += 1,
                        Err(_) => report.deferred_count += 1,
                    }
                    continue;
                }
                RemovedPeerWindow::Unstarted => {
                    if self
                        .ledger
                        .start_removed_peer_delivery_window(peer.clone(), now_ms)
                        .await
                        .is_err()
                    {
                        report.deferred_count += 1;
                        continue;
                    }
                }
                RemovedPeerWindow::Open => {}
            }
            plans.extend(
                record
                    .restricted_delivery
                    .iter()
                    .cloned()
                    .map(|delivery| (peer.clone(), delivery)),
            );
        }
        for (peer, delivery) in plans {
            match self
                .delivery
                .deliver_restricted_membership(&peer, &delivery)
                .await
            {
                Ok(()) => match self
                    .ledger
                    .confirm_restricted_membership_delivery(peer, delivery)
                    .await
                {
                    Ok(()) => report.completed_count += 1,
                    Err(_) => report.deferred_count += 1,
                },
                Err(RestrictedMembershipDeliveryError::Deferred) => {
                    report.deferred_count += 1;
                }
                Err(RestrictedMembershipDeliveryError::Rejected) => {
                    report.stable_failure_count += 1;
                }
            }
        }
        report
    }
}

#[async_trait]
impl DeliverRestrictedMembershipPort for DeliverRestrictedMembershipUseCase {
    async fn deliver_restricted_membership(&self) -> MembershipMaintenanceStepOutcome {
        let report = self.execute().await;
        if report.corrupt_count > 0 {
            MembershipMaintenanceStepOutcome::Corrupt
        } else if report.stable_failure_count > 0 {
            MembershipMaintenanceStepOutcome::StableFailure
        } else if report.deferred_count > 0 {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            MembershipMaintenanceStepOutcome::Completed
        }
    }
}

impl MembershipLedger {
    async fn start_removed_peer_delivery_window(
        &self,
        peer: DeviceId,
        now_ms: i64,
    ) -> Result<(), MembershipLedgerError> {
        self.compare_and_commit(move |record| {
            let relationship = record
                .peer_reconciliation
                .get_mut(&peer)
                .ok_or(MembershipLedgerError::Conflict)?;
            if relationship.updated_at_ms <= 0 {
                relationship.updated_at_ms = now_ms;
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// 只在该设备的核对记录与判定时一致时结束；期间任何变化都留给下一轮重新判定。
    async fn end_removed_peer_delivery(
        &self,
        observed: PeerReconciliationRecord,
    ) -> Result<(), MembershipLedgerError> {
        self.compare_and_commit(move |record| {
            if record.peer_reconciliation.get(&observed.peer_device_id) != Some(&observed) {
                return Err(MembershipLedgerError::Conflict);
            }
            record.peer_reconciliation.remove(&observed.peer_device_id);
            Ok(())
        })
        .await?;
        Ok(())
    }

    async fn confirm_restricted_membership_delivery(
        &self,
        peer: DeviceId,
        delivered: RestrictedMembershipDelivery,
    ) -> Result<(), MembershipLedgerError> {
        self.compare_and_commit(move |record| {
            let relationship = record
                .peer_reconciliation
                .get_mut(&peer)
                .ok_or(MembershipLedgerError::Conflict)?;
            let index = relationship
                .restricted_delivery
                .iter()
                .position(|candidate| candidate == &delivered)
                .ok_or(MembershipLedgerError::Conflict)?;
            relationship.restricted_delivery.remove(index);
            Ok(())
        })
        .await?;
        Ok(())
    }
}
