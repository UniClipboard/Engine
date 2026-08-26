use std::sync::Arc;

use crate::deps::{AdmissionOutboxDeliveryPort, AdmissionOutboxDeliveryResult};
use crate::space::membership::MembershipLedger;
use crate::space::membership::{
    MembershipMaintenanceReport, MembershipMaintenanceStepOutcome, RecoverSpaceAdmissionsPort,
};

pub(crate) struct RecoverSpaceAdmissionsUseCase {
    ledger: Arc<MembershipLedger>,
    delivery: Arc<dyn AdmissionOutboxDeliveryPort>,
}

impl RecoverSpaceAdmissionsUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        delivery: Arc<dyn AdmissionOutboxDeliveryPort>,
    ) -> Self {
        Self { ledger, delivery }
    }

    pub(crate) async fn execute(&self) -> MembershipMaintenanceReport {
        let mut report = MembershipMaintenanceReport::default();
        let records = match self.ledger.recoverable_admission_records().await {
            Ok(records) => records,
            Err(_) => {
                report.corrupt_count = 1;
                return report;
            }
        };
        for record in records {
            let message_ids = record
                .outboxes
                .iter()
                .filter(|message| !message.superseded)
                .map(|message| message.message_id)
                .collect::<Vec<_>>();
            for message_id in message_ids {
                let current = match self.ledger.load_admission_record(record.record_id).await {
                    Ok(Some(current)) => current,
                    Ok(None) | Err(_) => {
                        report.deferred_count += 1;
                        continue;
                    }
                };
                let Some(message) = current
                    .outboxes
                    .iter()
                    .find(|message| !message.superseded && message.message_id == message_id)
                else {
                    continue;
                };
                let result = self
                    .delivery
                    .deliver(current.record_id, message, None)
                    .await;
                match result {
                    Ok(AdmissionOutboxDeliveryResult::Persisted(acknowledgment))
                        if crate::space::admission::outbox::acknowledgment(message)
                            == acknowledgment =>
                    {
                        match self
                            .ledger
                            .settle_admission_outbox(
                                current.record_id,
                                current.record_version,
                                message.message_id,
                                acknowledgment,
                            )
                            .await
                        {
                            Ok(_) => report.completed_count += 1,
                            Err(_) => report.deferred_count += 1,
                        }
                    }
                    Ok(AdmissionOutboxDeliveryResult::Deferred) | Err(_) => {
                        report.deferred_count += 1;
                    }
                    Ok(AdmissionOutboxDeliveryResult::Rejected(rejected))
                        if rejected.purpose
                            == uc_core::membership::AdmissionOutboxPurpose::Rejected
                            && rejected.predecessor_message_id == Some(message.message_id) =>
                    {
                        let acknowledgment =
                            crate::space::admission::outbox::acknowledgment(&rejected);
                        if self
                            .ledger
                            .settle_admission_outbox(
                                current.record_id,
                                current.record_version,
                                message.message_id,
                                acknowledgment,
                            )
                            .await
                            .is_ok()
                        {
                            report.stable_failure_count += 1;
                        } else {
                            report.deferred_count += 1;
                        }
                    }
                    Ok(AdmissionOutboxDeliveryResult::InvitationConsume(result)) => {
                        let acknowledgment =
                            crate::space::admission::outbox::acknowledgment(message);
                        if self
                            .ledger
                            .settle_admission_outbox(
                                current.record_id,
                                current.record_version,
                                message.message_id,
                                acknowledgment,
                            )
                            .await
                            .is_err()
                        {
                            report.deferred_count += 1;
                        } else if matches!(
                            result,
                            crate::deps::InvitationConsumeDeliveryResult::Consumed
                        ) {
                            report.completed_count += 1;
                        } else {
                            report.stable_failure_count += 1;
                        }
                    }
                    Ok(AdmissionOutboxDeliveryResult::Persisted(_))
                    | Ok(AdmissionOutboxDeliveryResult::Rejected(_)) => {
                        report.stable_failure_count += 1;
                    }
                }
            }
        }
        report
    }
}

#[async_trait::async_trait]
impl RecoverSpaceAdmissionsPort for RecoverSpaceAdmissionsUseCase {
    async fn recover_space_admissions(&self) -> MembershipMaintenanceStepOutcome {
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
