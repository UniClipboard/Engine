use super::{ExecuteJoinerActivationError, JoinerActivationStateError};
use crate::space::admission::protocol::{
    AdmissionRecoveryReport, JoinerActivationOutcome, JoinerAdmissionService,
    SpaceAdmissionProtocol,
};
use crate::space::admission::{
    CompletePendingSpaceTransitionError, CurrentJoinStatus, JoinedSpace,
    QueryPendingSpaceTransitionError,
};

use super::model::JoinerActivationMutation;
use std::time::Instant;

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn recover_activation(
        &self,
    ) -> (AdmissionRecoveryReport, Option<JoinerActivationOutcome>) {
        let started = Instant::now();
        let mut report = AdmissionRecoveryReport::default();
        let loaded = match self.activation_state.load().await {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return (report, None),
            Err(error) => {
                record_state_error(&mut report, error);
                return (report, None);
            }
        };
        let (aggregate, token) = loaded.into_parts();
        let preparation = match aggregate.joiner_activation_preparation() {
            Some(preparation) => preparation,
            None => {
                report.recovery_required_count += 1;
                return (report, None);
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
                return (report, None);
            }
            Err(ExecuteJoinerActivationError::Unavailable { .. }) => {
                report.deferred_count += 1;
                return (report, None);
            }
        };
        let (transition_result, pending_exchange, outcome) = completed.into_parts();
        let transition = match aggregate.activate_complete(transition_result, pending_exchange) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return (report, None);
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
                tracing::info!(
                    target: "admission.performance",
                    phase = "joiner_activation_committed",
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    outcome = "ok",
                    "配对阶段完成"
                );
                return (report, Some(outcome));
            }
            Err(error) => record_state_error(&mut report, error),
        }
        (report, None)
    }
}

impl SpaceAdmissionProtocol {
    pub(crate) async fn has_pending_space_transition(
        &self,
    ) -> Result<bool, QueryPendingSpaceTransitionError> {
        self.execute_exclusively(async {
            self.joiner
                .activation_state
                .load()
                .await
                .map(|loaded| loaded.is_some())
                .map_err(QueryPendingSpaceTransitionError::state)
        })
        .await
    }

    pub(crate) async fn complete_pending_space_transition(
        &self,
    ) -> Result<CurrentJoinStatus, CompletePendingSpaceTransitionError> {
        self.execute_exclusively(async {
            let (report, outcome) = self.joiner.recover_activation().await;
            let outcome = outcome.ok_or_else(|| {
                if report.recovery_required_count > 0 {
                    CompletePendingSpaceTransitionError::state(anyhow::anyhow!(
                        "joiner activation requires recovery"
                    ))
                } else if report.deferred_count > 0 {
                    CompletePendingSpaceTransitionError::state(anyhow::anyhow!(
                        "joiner activation is temporarily unavailable"
                    ))
                } else {
                    CompletePendingSpaceTransitionError::JoinNotActive
                }
            })?;
            Ok(CurrentJoinStatus::Active {
                join_id: outcome.join_id,
                joined_space: JoinedSpace {
                    sponsor_device_id: outcome.sponsor_device_id,
                    sponsor_identity_fingerprint: outcome.sponsor_identity_fingerprint,
                    space_id: outcome.space_id,
                    self_device_id: outcome.self_device_id,
                    self_identity_fingerprint: outcome.self_identity_fingerprint,
                    migrated_records: outcome.migrated_records,
                    preserved_unreadable_records: outcome.preserved_unreadable_records,
                },
            })
        })
        .await
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
