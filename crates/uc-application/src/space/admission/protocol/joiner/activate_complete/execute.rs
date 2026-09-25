use super::{ExecuteJoinerActivationError, JoinerActivationStateError, JoinerMembershipStart};
use crate::space::admission::protocol::{
    AdmissionRecoveryReport, JoinerActivationOutcome, JoinerAdmissionService,
    SpaceAdmissionProtocol,
};
use crate::space::admission::{
    CompletePendingSpaceTransitionError, CurrentJoinStatus, QueryPendingSpaceTransitionError,
};
use crate::space::membership::MembershipLedgerError;
use uc_observability_contract::diagnostics::connectivity::{
    scope_pairing_work, AdmissionExchangeSide,
};
use uc_observability_contract::diagnostics::AdmissionObservationAction;

use super::model::JoinerActivationMutation;

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn recover_activation(
        &self,
    ) -> (AdmissionRecoveryReport, Option<JoinerActivationOutcome>) {
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
        self.observations
            .scope(
                *aggregate.admission_id().as_bytes(),
                scope_pairing_work(
                    AdmissionExchangeSide::Joiner,
                    Some(AdmissionObservationAction::Settle),
                    async {
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
                        let (transition_result, pending_exchange, outcome, membership) =
                            completed.into_parts();
                        // 成员事实先于准入终态提交；重启后重放同一激活时 Owner 不产生第二次变化。
                        if let Err(error) = self.establish_membership(membership).await {
                            record_membership_error(&mut report, &error);
                            return (report, None);
                        }
                        let transition = match aggregate
                            .activate_complete(transition_result, pending_exchange)
                        {
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
                                return (report, Some(outcome));
                            }
                            Err(error) => record_state_error(&mut report, error),
                        }
                        (report, None)
                    },
                ),
            )
            .await
    }
}

impl JoinerAdmissionService {
    /// 目标控制世代已生效：以新世代的成员记录重新加载 Owner，再建立本机成员状态。
    async fn establish_membership(
        &self,
        membership: JoinerMembershipStart,
    ) -> Result<(), MembershipLedgerError> {
        self.members.reload().await?;
        let JoinerMembershipStart {
            space_id,
            local_device_id,
            local_member,
            history,
        } = membership;
        self.members
            .commit(move |draft| {
                draft.join_space(&space_id, history, local_device_id, local_member)
            })
            .await
            .map(|_| ())
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
            Ok(CurrentJoinStatus::Processing {
                join_id: outcome.join_id,
                target_space_id: outcome.space_id,
                sponsor_device_id: outcome.sponsor_device_id,
                sponsor_identity_fingerprint: outcome.sponsor_identity_fingerprint,
                peer_upgrade_required: false,
            })
        })
        .await
    }
}

fn record_membership_error(report: &mut AdmissionRecoveryReport, error: &MembershipLedgerError) {
    let error_kind = match error {
        MembershipLedgerError::Locked => "locked",
        MembershipLedgerError::Unavailable { .. } => "unavailable",
        MembershipLedgerError::Conflict => "conflict",
        MembershipLedgerError::Corrupt { .. } => "corrupt",
        MembershipLedgerError::RecoveryRequired => "recovery_required",
    };
    tracing::warn!(error_kind, "加入方激活后建立本机成员状态失败");
    match error {
        MembershipLedgerError::Locked | MembershipLedgerError::Unavailable { .. } => {
            report.deferred_count += 1
        }
        // 当前成员状态属于其他 Space 或其他成员实例，或无法校验：不能在其上完成本次加入。
        MembershipLedgerError::Conflict
        | MembershipLedgerError::Corrupt { .. }
        | MembershipLedgerError::RecoveryRequired => report.recovery_required_count += 1,
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
