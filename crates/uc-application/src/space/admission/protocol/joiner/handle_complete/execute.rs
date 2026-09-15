use uc_core::membership::{JoinerAdmission, SpaceAdmissionEnvelopeV1};

use crate::space::admission::protocol::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryService,
    JoinerAdmissionService, JoinerReplyHandlingOutcome,
};

use super::PrepareJoinerActivationError;

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn handle_complete(
        &self,
        recovery: &AdmissionRecoveryService,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        reply: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        notify_upgrade_cleared: bool,
    ) -> JoinerReplyHandlingOutcome {
        let preparation = match aggregate.joiner_complete_preparation() {
            Some(preparation) => preparation,
            None => {
                report.recovery_required_count += 1;
                return JoinerReplyHandlingOutcome::NoImmediateWork;
            }
        };
        let activation = match self
            .prepare_activation
            .prepare(aggregate.admission_id(), preparation, &reply)
            .await
        {
            Ok(activation) => activation,
            Err(PrepareJoinerActivationError::Invalid { .. }) => {
                report.recovery_required_count += 1;
                return JoinerReplyHandlingOutcome::NoImmediateWork;
            }
            Err(PrepareJoinerActivationError::Unavailable { .. }) => {
                report.deferred_count += 1;
                return JoinerReplyHandlingOutcome::NoImmediateWork;
            }
        };
        let transition = match aggregate.accept_complete(
            reply,
            canonical_digest,
            activation.into_transition(),
        ) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return JoinerReplyHandlingOutcome::NoImmediateWork;
            }
        };
        let commit_result = recovery
            .commit_recovery_with_optional_notification(token, transition, notify_upgrade_cleared)
            .await;
        match commit_result {
            Ok(_) => {
                report.advanced_count += 1;
                self.space_transition_changes.send_replace(());
                JoinerReplyHandlingOutcome::AwaitingSpaceTransition
            }
            Err(error) => {
                recovery.record_state_error(report, error);
                JoinerReplyHandlingOutcome::NoImmediateWork
            }
        }
    }
}
