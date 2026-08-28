use super::{ExecuteJoinerActivationError, JoinerActivationStateError};
use crate::space::admission::protocol::{AdmissionRecoveryReport, JoinerAdmissionService};

use super::model::JoinerActivationMutation;

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn recover_activation(
        &self,
    ) -> AdmissionRecoveryReport {
        let mut report = AdmissionRecoveryReport::default();
        let loaded = match self.activation_state.load().await {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return report,
            Err(error) => {
                record_state_error(&mut report, error);
                return report;
            }
        };
        let (aggregate, token) = loaded.into_parts();
        let preparation = match aggregate.joiner_activation_preparation() {
            Some(preparation) => preparation,
            None => {
                report.recovery_required_count += 1;
                return report;
            }
        };
        let completed = match self
            .execute_activation
            .execute(aggregate.admission_id(), preparation)
            .await
        {
            Ok(completed) => completed,
            Err(ExecuteJoinerActivationError::Invalid { .. }) => {
                report.recovery_required_count += 1;
                return report;
            }
            Err(ExecuteJoinerActivationError::Unavailable { .. }) => {
                report.deferred_count += 1;
                return report;
            }
        };
        let (transition_result, pending_exchange) = completed.into_parts();
        let transition = match aggregate.activate_complete(transition_result, pending_exchange) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return report;
            }
        };
        match self
            .activation_state
            .commit(token, JoinerActivationMutation::new(transition))
            .await
        {
            Ok(()) => {
                report.advanced_count += 1;
                self.maintenance_wake.wake();
            }
            Err(error) => record_state_error(&mut report, error),
        }
        report
    }
}

fn record_state_error(report: &mut AdmissionRecoveryReport, error: JoinerActivationStateError) {
    match error {
        JoinerActivationStateError::Locked { .. }
        | JoinerActivationStateError::Unavailable { .. } => report.deferred_count += 1,
        JoinerActivationStateError::StateChanged { .. } => report.deferred_count += 1,
        JoinerActivationStateError::RecoveryRequired { .. } => report.recovery_required_count += 1,
    }
}
