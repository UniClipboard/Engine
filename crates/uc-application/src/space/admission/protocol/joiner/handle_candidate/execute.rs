use uc_core::membership::{JoinerAdmission, SpaceAdmissionEnvelopeV1};

use super::PrepareJoinerCandidateError;
use crate::space::admission::protocol::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryService,
    JoinerAdmissionService,
};

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn handle_candidate(
        &self,
        recovery: &AdmissionRecoveryService,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        reply: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) {
        let preparation = match aggregate.joiner_candidate_preparation() {
            Some(preparation) => preparation,
            None => {
                report.recovery_required_count += 1;
                return;
            }
        };
        let prepare_started = std::time::Instant::now();
        let prepared_result = self.prepare_candidate.prepare(preparation, &reply).await;
        crate::space::admission::protocol::record_performance_phase(
            "joiner_prepare_candidate",
            prepare_started,
            prepared_result.is_ok(),
        );
        let prepared = match prepared_result {
            Ok(prepared) => prepared,
            Err(PrepareJoinerCandidateError::Unavailable { .. }) => {
                report.deferred_count += 1;
                return;
            }
            Err(
                PrepareJoinerCandidateError::Invalid
                | PrepareJoinerCandidateError::InvalidSource { .. },
            ) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        let (staged_input, verified_history, staged_target, prepared_exchange) =
            prepared.into_parts();
        let transition = match aggregate.accept_candidate(reply, canonical_digest, staged_input) {
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
        let transition =
            match aggregate.prepare_candidate(verified_history, staged_target, prepared_exchange) {
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
