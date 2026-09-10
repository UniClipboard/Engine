use super::model::AdmissionRecoveryReport;
use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AuthenticatedAdmissionReply,
    LoadedPendingAdmission, PendingAdmissionRecoveryStateError, SpaceAdmissionTransportError,
};
use crate::space::admission::protocol::{
    AdmissionRecoveryService, JoinerAdmissionService, SpaceAdmissionProtocol,
};
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};
use uc_core::membership::{
    AdmissionPendingRecovery, AdmissionRecoveryCategory, JoinerAdmission,
    SpaceAdmissionMessageKind, SpaceAdmissionRejectionReason,
};
use uc_observability_contract::diagnostics::connectivity::{
    record_admission_recovery_decision, ExchangeFailure, RecoveryDecision, RecoveryDeferral,
    RecoveryProblem, RecoveryTrigger, RejectionCause, StateFailure,
};
use uc_observability_contract::diagnostics::{
    DiagnosticErrorType, ObservationContext, SpaceAdmissionObservationOutcome,
};

#[derive(Clone, Copy)]
enum RecoveryChannel {
    Initial,
    Continuation,
}

impl SpaceAdmissionProtocol {
    pub(crate) async fn recover_pending(
        &self,
        trigger: AdmissionRecoveryTrigger,
    ) -> AdmissionRecoveryReport {
        self.recovery.recover_pending(&self.joiner, trigger).await
    }
}

impl AdmissionRecoveryService {
    async fn recover_pending(
        &self,
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
    ) -> AdmissionRecoveryReport {
        // 恢复执行独占自己的入口，不能跨网络等待占用本机动作锁。
        // 状态提交仍由持久仓库校验版本，拒绝覆盖并发取消或替换。
        let _recovery = self.execution_lock.lock().await;
        let mut report = AdmissionRecoveryReport::default();
        let loaded = match self.state.load(trigger).await {
            Ok(loaded) => loaded,
            Err(error) => {
                let decision = match &error {
                    PendingAdmissionRecoveryStateError::RecoveryRequired => {
                        RecoveryDecision::RequiresRecovery(Some(RecoveryProblem::CorruptState))
                    }
                    PendingAdmissionRecoveryStateError::Locked => RecoveryDecision::Deferred(Some(
                        RecoveryDeferral::State(StateFailure::Locked),
                    )),
                    PendingAdmissionRecoveryStateError::Unavailable => RecoveryDecision::Deferred(
                        Some(RecoveryDeferral::State(StateFailure::Unavailable)),
                    ),
                    PendingAdmissionRecoveryStateError::StateChanged => RecoveryDecision::Deferred(
                        Some(RecoveryDeferral::State(StateFailure::Changed)),
                    ),
                };
                record_admission_recovery_decision(
                    &ObservationContext::capture(),
                    diagnostic_trigger(trigger),
                    decision,
                );
                let outcome = match &error {
                    PendingAdmissionRecoveryStateError::RecoveryRequired => {
                        SpaceAdmissionObservationOutcome::Failed(DiagnosticErrorType::Corrupt)
                    }
                    PendingAdmissionRecoveryStateError::Locked
                    | PendingAdmissionRecoveryStateError::Unavailable
                    | PendingAdmissionRecoveryStateError::StateChanged => {
                        SpaceAdmissionObservationOutcome::Deferred
                    }
                };
                joiner.observations.finish_all(outcome);
                self.record_state_error(&mut report, error);
                return report;
            }
        };

        for loaded_admission in loaded {
            let (aggregate, commit_token) = loaded_admission.into_parts();
            if aggregate.pending_recovery().is_none() && aggregate.invitation_resolution().is_none()
            {
                continue;
            }
            let observation_before = report;
            let observation_material = *aggregate.admission_id().as_bytes();
            let was_cancelling = aggregate.is_cancelling();
            let context = joiner
                .observations
                .scope(observation_material, async {
                    ObservationContext::capture()
                })
                .await;
            let mut decision_hint = None;
            let finish_observation = |after, hint| {
                if let Some(decision) =
                    actual_recovery_decision(observation_before, after, was_cancelling, hint)
                {
                    record_admission_recovery_decision(
                        &context,
                        diagnostic_trigger(trigger),
                        decision,
                    );
                }
                finish_observation_after_recovery(
                    joiner,
                    observation_material,
                    was_cancelling,
                    observation_before,
                    after,
                );
            };
            if aggregate.invitation_resolution().is_some() {
                joiner
                    .recover_invitation_resolution(self, &mut report, aggregate, commit_token)
                    .await;
                finish_observation(report, decision_hint);
                continue;
            }
            let Some(recovery) = aggregate.pending_recovery() else {
                continue;
            };
            let (channel_kind, established) = joiner
                .observations
                .scope(observation_material, async {
                    match recovery {
                        AdmissionPendingRecovery::Initial {
                            encrypted_password_equivalent,
                            pending_exchange,
                        } => (
                            RecoveryChannel::Initial,
                            self.transport
                                .establish_initial(
                                    aggregate.admission_id(),
                                    pending_exchange.route(),
                                    encrypted_password_equivalent,
                                )
                                .await,
                        ),
                        AdmissionPendingRecovery::Continuation {
                            peer_binding,
                            continuation_credential,
                            pending_exchange,
                        } => (
                            RecoveryChannel::Continuation,
                            self.transport
                                .resume(
                                    aggregate.admission_id(),
                                    pending_exchange.route(),
                                    peer_binding,
                                    continuation_credential,
                                )
                                .await,
                        ),
                    }
                })
                .await;

            let mut exchange = match established {
                Ok(exchange) => exchange,
                Err(error) => {
                    decision_hint = Some(connection_decision(channel_kind, error));
                    self.record_connection_failure(
                        &mut report,
                        channel_kind,
                        aggregate,
                        commit_token,
                        error,
                    )
                    .await;
                    finish_observation(report, decision_hint);
                    continue;
                }
            };

            let loaded = match channel_kind {
                RecoveryChannel::Initial => {
                    let peer_binding = exchange.peer_binding();
                    let Some(continuation) = exchange.take_newly_established_continuation() else {
                        decision_hint = Some(RecoveryDecision::RequiresRecovery(Some(
                            RecoveryProblem::MissingCredential,
                        )));
                        self.save_recovery_required(
                            &mut report,
                            aggregate,
                            commit_token,
                            AdmissionRecoveryCategory::MissingKey,
                        )
                        .await;
                        finish_observation(report, decision_hint);
                        continue;
                    };
                    let transition =
                        match aggregate.with_authenticated_channel(peer_binding, continuation) {
                            Ok(transition) => transition,
                            Err(_) => {
                                report.recovery_required_count += 1;
                                finish_observation(report, decision_hint);
                                continue;
                            }
                        };
                    match self.commit_recovery(commit_token, transition).await {
                        Ok(loaded) => {
                            report.advanced_count += 1;
                            loaded
                        }
                        Err(error) => {
                            self.record_state_error(&mut report, error);
                            finish_observation(report, decision_hint);
                            continue;
                        }
                    }
                }
                RecoveryChannel::Continuation => {
                    LoadedPendingAdmission::new(aggregate, commit_token)
                }
            };

            let (aggregate, commit_token) = loaded.into_parts();
            let Some(pending_exchange) = aggregate.pending_exchange() else {
                report.recovery_required_count += 1;
                finish_observation(report, decision_hint);
                continue;
            };
            let exchanged = joiner
                .observations
                .scope(
                    observation_material,
                    uc_observability_contract::diagnostics::scope_admission_action(
                        crate::space::admission::observation::message_action(
                            pending_exchange.request_envelope().kind(),
                        ),
                        exchange.exchange(pending_exchange.request_envelope()),
                    ),
                )
                .await;
            match exchanged {
                Ok(reply) => {
                    self.commit_joiner_reply(joiner, &mut report, aggregate, commit_token, reply)
                        .await;
                }
                Err(SpaceAdmissionTransportError::PeerUpgradeRequired) => {
                    decision_hint = Some(RecoveryDecision::Rejected(Some(
                        RejectionCause::PeerUpgradeRequired,
                    )));
                    self.save_peer_upgrade_result(&mut report, aggregate, commit_token)
                        .await;
                }
                Err(error) => {
                    decision_hint = Some(RecoveryDecision::Deferred(Some(
                        RecoveryDeferral::Exchange(exchange_failure(error)),
                    )));
                    report.deferred_count += 1;
                }
            }
            finish_observation(report, decision_hint);
        }

        report
    }

    async fn record_connection_failure(
        &self,
        report: &mut AdmissionRecoveryReport,
        channel: RecoveryChannel,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        error: SpaceAdmissionTransportError,
    ) {
        match (channel, error) {
            (RecoveryChannel::Initial, SpaceAdmissionTransportError::InvitationUnavailable) => {
                self.save_initial_rejection(
                    report,
                    aggregate,
                    token,
                    SpaceAdmissionRejectionReason::InvitationUnavailable,
                )
                .await;
            }
            (RecoveryChannel::Initial, SpaceAdmissionTransportError::AuthenticationRejected) => {
                self.save_initial_rejection(
                    report,
                    aggregate,
                    token,
                    SpaceAdmissionRejectionReason::AuthenticationRejected,
                )
                .await;
            }
            (RecoveryChannel::Initial, SpaceAdmissionTransportError::PeerUpgradeRequired) => {
                self.save_peer_upgrade_result(report, aggregate, token)
                    .await;
            }
            (_, SpaceAdmissionTransportError::ProtocolRejected) => {
                self.save_recovery_required(
                    report,
                    aggregate,
                    token,
                    AdmissionRecoveryCategory::ProtocolConflict,
                )
                .await;
            }
            (
                RecoveryChannel::Continuation,
                SpaceAdmissionTransportError::AuthenticationRejected,
            ) => {
                self.save_recovery_required(
                    report,
                    aggregate,
                    token,
                    AdmissionRecoveryCategory::MissingKey,
                )
                .await;
            }
            _ => report.deferred_count += 1,
        }
    }

    async fn save_initial_rejection(
        &self,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        reason: SpaceAdmissionRejectionReason,
    ) {
        let transition = match aggregate.reject_before_authentication(reason) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        match self.commit_recovery_and_notify(token, transition).await {
            Ok(_) => report.rejected_count += 1,
            Err(error) => self.record_state_error(report, error),
        }
    }

    async fn save_peer_upgrade_result(
        &self,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
    ) {
        if aggregate.peer_upgrade_required() {
            report.peer_upgrade_required_count += 1;
            return;
        }
        let is_initial_request = aggregate.pending_exchange().is_some_and(|exchange| {
            exchange.request_envelope().kind() == SpaceAdmissionMessageKind::JoinRequest
        });
        let transition = match if is_initial_request {
            aggregate.reject_peer_upgrade()
        } else {
            aggregate.mark_peer_upgrade_required()
        } {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        match self.commit_recovery_and_notify(token, transition).await {
            Ok(_) => {
                report.peer_upgrade_required_count += 1;
                if is_initial_request {
                    report.rejected_count += 1;
                }
            }
            Err(error) => self.record_state_error(report, error),
        }
    }

    async fn save_recovery_required(
        &self,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        category: AdmissionRecoveryCategory,
    ) {
        let transition = match aggregate.require_recovery(category) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return;
            }
        };
        match self.commit_recovery(token, transition).await {
            Ok(_) => report.recovery_required_count += 1,
            Err(error) => self.record_state_error(report, error),
        }
    }

    async fn commit_joiner_reply(
        &self,
        joiner: &JoinerAdmissionService,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
        reply: AuthenticatedAdmissionReply,
    ) {
        let (reply, canonical_digest) = reply.into_parts();
        match reply.kind() {
            SpaceAdmissionMessageKind::Rejected => {
                let transition = match aggregate.accept_rejection(reply, canonical_digest) {
                    Ok(transition) => transition,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                match self.commit_recovery_and_notify(token, transition).await {
                    Ok(_) => report.rejected_count += 1,
                    Err(error) => self.record_state_error(report, error),
                }
            }
            SpaceAdmissionMessageKind::Candidate => {
                joiner
                    .handle_candidate(self, report, aggregate, token, reply, canonical_digest)
                    .await;
            }
            SpaceAdmissionMessageKind::Commit => {
                let notify_upgrade_cleared = aggregate.peer_upgrade_required();
                joiner
                    .handle_commit(
                        self,
                        report,
                        aggregate,
                        token,
                        reply,
                        canonical_digest,
                        notify_upgrade_cleared,
                    )
                    .await;
            }
            SpaceAdmissionMessageKind::Complete => {
                let notify_upgrade_cleared = aggregate.peer_upgrade_required();
                joiner
                    .handle_complete(
                        self,
                        report,
                        aggregate,
                        token,
                        reply,
                        canonical_digest,
                        notify_upgrade_cleared,
                    )
                    .await;
            }
            SpaceAdmissionMessageKind::Settled => {
                let notify_upgrade_cleared = aggregate.peer_upgrade_required();
                joiner
                    .handle_settled(
                        self,
                        report,
                        aggregate,
                        token,
                        reply,
                        canonical_digest,
                        notify_upgrade_cleared,
                    )
                    .await;
            }
            _ => {
                self.save_recovery_required(
                    report,
                    aggregate,
                    token,
                    AdmissionRecoveryCategory::ProtocolConflict,
                )
                .await;
            }
        }
    }
}

fn finish_observation_after_recovery(
    joiner: &JoinerAdmissionService,
    material: [u8; 32],
    was_cancelling: bool,
    before: AdmissionRecoveryReport,
    after: AdmissionRecoveryReport,
) {
    let outcome = if after.recovery_required_count > before.recovery_required_count {
        Some(SpaceAdmissionObservationOutcome::Failed(
            DiagnosticErrorType::Corrupt,
        ))
    } else if after.peer_upgrade_required_count > before.peer_upgrade_required_count {
        Some(SpaceAdmissionObservationOutcome::Rejected)
    } else if after.rejected_count > before.rejected_count {
        Some(if was_cancelling {
            SpaceAdmissionObservationOutcome::Cancelled
        } else {
            SpaceAdmissionObservationOutcome::Rejected
        })
    } else if after.deferred_count > before.deferred_count {
        Some(SpaceAdmissionObservationOutcome::Deferred)
    } else {
        None
    };
    if let Some(outcome) = outcome {
        joiner.observations.finish(material, outcome);
    }
}

fn diagnostic_trigger(trigger: AdmissionRecoveryTrigger) -> RecoveryTrigger {
    match trigger {
        AdmissionRecoveryTrigger::Startup => RecoveryTrigger::Startup,
        AdmissionRecoveryTrigger::Resume => RecoveryTrigger::Resume,
        AdmissionRecoveryTrigger::Periodic => RecoveryTrigger::Periodic,
        AdmissionRecoveryTrigger::StateChanged => RecoveryTrigger::StateChanged,
        AdmissionRecoveryTrigger::PeerOnline(_) => RecoveryTrigger::PeerOnline,
    }
}
fn exchange_failure(error: SpaceAdmissionTransportError) -> ExchangeFailure {
    match error {
        SpaceAdmissionTransportError::AuthenticationRejected => {
            ExchangeFailure::AuthenticationRejected
        }
        SpaceAdmissionTransportError::ProtocolRejected => ExchangeFailure::ProtocolRejected,
        SpaceAdmissionTransportError::InvitationUnavailable => {
            ExchangeFailure::InvitationUnavailable
        }
        SpaceAdmissionTransportError::Unavailable => ExchangeFailure::Unavailable,
        SpaceAdmissionTransportError::Deferred => ExchangeFailure::Deferred,
        SpaceAdmissionTransportError::PeerUpgradeRequired => ExchangeFailure::PeerUpgradeRequired,
    }
}
fn connection_decision(
    channel: RecoveryChannel,
    error: SpaceAdmissionTransportError,
) -> RecoveryDecision {
    match (channel, error) {
        (RecoveryChannel::Initial, SpaceAdmissionTransportError::AuthenticationRejected) => {
            RecoveryDecision::Rejected(Some(RejectionCause::AuthenticationRejected))
        }
        (RecoveryChannel::Initial, SpaceAdmissionTransportError::InvitationUnavailable) => {
            RecoveryDecision::Rejected(Some(RejectionCause::InvitationUnavailable))
        }
        (RecoveryChannel::Initial, SpaceAdmissionTransportError::PeerUpgradeRequired) => {
            RecoveryDecision::Rejected(Some(RejectionCause::PeerUpgradeRequired))
        }
        (_, SpaceAdmissionTransportError::ProtocolRejected) => {
            RecoveryDecision::RequiresRecovery(Some(RecoveryProblem::ProtocolConflict))
        }
        (RecoveryChannel::Continuation, SpaceAdmissionTransportError::AuthenticationRejected) => {
            RecoveryDecision::RequiresRecovery(Some(RecoveryProblem::MissingCredential))
        }
        (_, error) => {
            RecoveryDecision::Deferred(Some(RecoveryDeferral::Connect(exchange_failure(error))))
        }
    }
}

// 先尊重实际保存结果，避免把“本来准备拒绝但保存失败”写成已经拒绝。
fn actual_recovery_decision(
    before: AdmissionRecoveryReport,
    after: AdmissionRecoveryReport,
    cancelling: bool,
    hint: Option<RecoveryDecision>,
) -> Option<RecoveryDecision> {
    if after.recovery_required_count > before.recovery_required_count {
        return Some(match hint {
            Some(decision @ RecoveryDecision::RequiresRecovery(_)) => decision,
            _ => RecoveryDecision::RequiresRecovery(None),
        });
    }
    if after.peer_upgrade_required_count > before.peer_upgrade_required_count
        || after.rejected_count > before.rejected_count
    {
        if cancelling && after.rejected_count > before.rejected_count {
            return Some(RecoveryDecision::Cancelled);
        }
        return Some(match hint {
            Some(decision @ RecoveryDecision::Rejected(_)) => decision,
            _ => RecoveryDecision::Rejected(None),
        });
    }
    if after.deferred_count > before.deferred_count {
        return Some(match hint {
            Some(decision @ RecoveryDecision::Deferred(_)) => decision,
            _ => RecoveryDecision::Deferred(None),
        });
    }
    None
}

#[async_trait::async_trait]
impl RecoverSpaceAdmissionsPort for SpaceAdmissionProtocol {
    async fn recover_space_admissions(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        let trigger = match trigger {
            MembershipMaintenanceTrigger::Startup => AdmissionRecoveryTrigger::Startup,
            MembershipMaintenanceTrigger::Resume => AdmissionRecoveryTrigger::Resume,
            MembershipMaintenanceTrigger::Periodic => AdmissionRecoveryTrigger::Periodic,
            MembershipMaintenanceTrigger::StateChanged => AdmissionRecoveryTrigger::StateChanged,
            MembershipMaintenanceTrigger::PeerOnline(device_id) => {
                AdmissionRecoveryTrigger::PeerOnline(*device_id)
            }
        };
        let report = self.recover_pending(trigger).await;
        if report.recovery_required_count > 0 {
            MembershipMaintenanceStepOutcome::Corrupt
        } else if report.peer_upgrade_required_count > 0 || report.rejected_count > 0 {
            MembershipMaintenanceStepOutcome::StableFailure
        } else if report.deferred_count > 0 {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            MembershipMaintenanceStepOutcome::Completed
        }
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn failed_persistence_overrides_the_intended_rejection_in_diagnostics() {
        let before = AdmissionRecoveryReport::default();
        let after = AdmissionRecoveryReport {
            deferred_count: 1,
            ..before
        };
        let intended = RecoveryDecision::Rejected(Some(RejectionCause::AuthenticationRejected));
        assert_eq!(
            actual_recovery_decision(before, after, false, Some(intended)),
            Some(RecoveryDecision::Deferred(None))
        );
    }
    #[test]
    fn exchange_rejection_that_stays_pending_keeps_the_actual_wait_decision() {
        let before = AdmissionRecoveryReport::default();
        let after = AdmissionRecoveryReport {
            deferred_count: 1,
            ..before
        };
        let actual = RecoveryDecision::Deferred(Some(RecoveryDeferral::Exchange(
            ExchangeFailure::AuthenticationRejected,
        )));
        assert_eq!(
            actual_recovery_decision(before, after, false, Some(actual)),
            Some(actual)
        );
        assert_eq!(actual_recovery_decision(before, before, false, None), None);
    }
}
