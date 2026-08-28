use uc_core::membership::{SpaceAdmissionAggregate, SpaceAdmissionEnvelopeV1};

use crate::space::admission::protocol::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryService,
    JoinerAdmissionService,
};

use super::PrepareJoinerAppliedError;

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn handle_commit(
        &self,
        recovery: &AdmissionRecoveryService,
        report: &mut AdmissionRecoveryReport,
        aggregate: SpaceAdmissionAggregate,
        token: AdmissionRecoveryCommitToken,
        reply: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) {
        let transition = match aggregate.accept_commit(reply, canonical_digest) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        let committed = match recovery.commit_recovery(token, transition).await {
            Ok(committed) => committed,
            Err(error) => {
                recovery.record_state_error(report, error);
                return;
            }
        };
        report.advanced_count += 1;
        let (aggregate, token) = committed.into_parts();
        let preparation = match aggregate.joiner_applied_preparation() {
            Some(preparation) => preparation,
            None => {
                report.recovery_required_count += 1;
                return;
            }
        };
        let material = match self
            .prepare_applied
            .prepare(aggregate.admission_id(), preparation)
            .await
        {
            Ok(material) => material,
            Err(PrepareJoinerAppliedError::Invalid { .. }) => {
                report.recovery_required_count += 1;
                return;
            }
            Err(PrepareJoinerAppliedError::Unavailable { .. }) => {
                report.deferred_count += 1;
                return;
            }
        };
        let transition = match aggregate.apply_commit(material.into_pending_exchange()) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        match recovery.commit_recovery(token, transition).await {
            Ok(_) => {
                report.advanced_count += 1;
                self.maintenance_wake.wake();
            }
            Err(error) => recovery.record_state_error(report, error),
        }
    }
}
