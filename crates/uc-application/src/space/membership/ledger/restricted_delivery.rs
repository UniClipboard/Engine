use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;

use crate::space::membership::{
    DeliverRestrictedMembershipPort, MembershipMaintenanceReport, MembershipMaintenanceStepOutcome,
};

use super::{MembershipLedger, MembershipLedgerError, RestrictedMembershipDelivery};

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

pub(crate) struct DeliverRestrictedMembershipUseCase {
    ledger: Arc<MembershipLedger>,
    delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
}

impl DeliverRestrictedMembershipUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
    ) -> Self {
        Self { ledger, delivery }
    }

    pub(crate) async fn execute(&self) -> MembershipMaintenanceReport {
        let mut report = MembershipMaintenanceReport::default();
        let plans = match self.ledger.load_verified().await {
            Ok(snapshot) => snapshot
                .record()
                .peer_reconciliation
                .iter()
                .flat_map(|(peer, record)| {
                    record
                        .restricted_delivery
                        .iter()
                        .cloned()
                        .map(|delivery| (peer.clone(), delivery))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
            Err(_) => {
                report.corrupt_count = 1;
                return report;
            }
        };
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
