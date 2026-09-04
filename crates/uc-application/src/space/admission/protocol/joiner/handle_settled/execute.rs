use uc_core::membership::{JoinerAdmission, SpaceAdmissionEnvelopeV1};

use crate::space::admission::protocol::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryService,
    JoinerAdmissionService,
};

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn handle_settled(
        &self,
        recovery: &AdmissionRecoveryService,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        reply: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) {
        let transition = match aggregate.accept_settled(reply, canonical_digest) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        match recovery.commit_recovery(token, transition).await {
            Ok(_) => match self.re_pairing.resolve_after_successful_pairing().await {
                Ok(()) => report.advanced_count += 1,
                Err(_) => report.deferred_count += 1,
            },
            Err(error) => recovery.record_state_error(report, error),
        }
    }
}
