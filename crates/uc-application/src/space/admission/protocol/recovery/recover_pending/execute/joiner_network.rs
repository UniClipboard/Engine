use super::super::model::{AdmissionRecoveryDisposition, AdmissionRecoveryReport};
use super::super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AuthenticatedAdmissionExchangePort,
    AuthenticatedAdmissionReply, LoadedPendingAdmission, SpaceAdmissionTransportError,
};
use super::{
    connection_decision, exchange_failure, joiner_reply_work_outcome, AuthenticatedChannelProgress,
    JoinerRecoveryObservation, RecoveryChannel, MAX_IMMEDIATE_EXCHANGES_PER_ADMISSION,
};
use crate::space::admission::observation::message_action;
use crate::space::admission::protocol::{
    AdmissionRecoveryService, JoinerAdmissionService, JoinerReplyHandlingOutcome,
};
use uc_core::membership::{
    AdmissionPendingRecovery, AdmissionRecoveryCategory, JoinerAdmission, SpaceAdmissionMessageKind,
};
use uc_observability_contract::diagnostics::connectivity::{
    scope_pairing_work, AdmissionExchangeSide, LocalWorkObservation, LocalWorkStep,
    RecoveryDecision, RecoveryDeferral, RecoveryProblem, RejectionCause,
};
use uc_observability_contract::diagnostics::scope_admission_action;

impl AdmissionRecoveryService {
    pub(super) async fn recover_joiner_network_work(
        &self,
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
        pending_admissions: Vec<LoadedPendingAdmission>,
        report: &mut AdmissionRecoveryReport,
    ) {
        'pending_admissions: for mut loaded in pending_admissions {
            for exchange_index in 0..MAX_IMMEDIATE_EXCHANGES_PER_ADMISSION {
                let progress =
                    Box::pin(self.recover_joiner_exchange(joiner, trigger, loaded, report)).await;
                match progress {
                    JoinerReplyHandlingOutcome::Continue(next)
                        if exchange_index + 1 < MAX_IMMEDIATE_EXCHANGES_PER_ADMISSION =>
                    {
                        loaded = next;
                    }
                    JoinerReplyHandlingOutcome::Continue(_) => {
                        joiner.maintenance_wake.wake();
                        report.disposition = AdmissionRecoveryDisposition::YieldMaintenance;
                        break 'pending_admissions;
                    }
                    JoinerReplyHandlingOutcome::AwaitingSpaceTransition => {
                        report.disposition = AdmissionRecoveryDisposition::YieldMaintenance;
                        break 'pending_admissions;
                    }
                    JoinerReplyHandlingOutcome::PairingFinished => {
                        // 最终确认已经持久化，先结束当前维护轮次，让调用方立即观察到配对完成。
                        // 普通成员维护由下一轮继续，不能再次插入准入协议内部。
                        joiner.maintenance_wake.wake();
                        report.disposition = AdmissionRecoveryDisposition::YieldMaintenance;
                        break 'pending_admissions;
                    }
                    JoinerReplyHandlingOutcome::NoImmediateWork => break,
                }
            }
        }
    }

    async fn recover_joiner_exchange(
        &self,
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
        loaded: LoadedPendingAdmission,
        report: &mut AdmissionRecoveryReport,
    ) -> JoinerReplyHandlingOutcome {
        let (aggregate, commit_token) = loaded.into_parts();
        if aggregate.pending_recovery().is_none() && aggregate.invitation_resolution().is_none() {
            return JoinerReplyHandlingOutcome::NoImmediateWork;
        }

        let observation =
            JoinerRecoveryObservation::begin(joiner, trigger, &aggregate, *report).await;
        let observation_material = observation.material;
        let (progress, decision_hint) = if aggregate.invitation_resolution().is_some() {
            joiner
                .observations
                .scope(
                    observation_material,
                    scope_pairing_work(
                        AdmissionExchangeSide::Joiner,
                        None,
                        joiner.recover_invitation_resolution(self, report, aggregate, commit_token),
                    ),
                )
                .await;
            (JoinerReplyHandlingOutcome::NoImmediateWork, None)
        } else {
            Box::pin(self.recover_joiner_protocol_exchange(
                joiner,
                observation_material,
                aggregate,
                commit_token,
                report,
            ))
            .await
        };

        observation.finish(joiner, *report, decision_hint);
        progress
    }

    async fn recover_joiner_protocol_exchange(
        &self,
        joiner: &JoinerAdmissionService,
        observation_material: [u8; 32],
        aggregate: JoinerAdmission,
        commit_token: AdmissionRecoveryCommitToken,
        report: &mut AdmissionRecoveryReport,
    ) -> (JoinerReplyHandlingOutcome, Option<RecoveryDecision>) {
        let Some(recovery) = aggregate.pending_recovery() else {
            return (JoinerReplyHandlingOutcome::NoImmediateWork, None);
        };
        let (channel, established) = joiner
            .observations
            .scope(observation_material, async {
                match recovery {
                    AdmissionPendingRecovery::Initial {
                        encrypted_password_equivalent,
                        pending_exchange,
                    } => (
                        RecoveryChannel::Initial,
                        match aggregate.attempt_timeline() {
                            Some(attempt_timeline) => {
                                self.transport
                                    .establish_initial(
                                        aggregate.admission_id(),
                                        attempt_timeline,
                                        pending_exchange.route(),
                                        encrypted_password_equivalent,
                                    )
                                    .await
                            }
                            None => Err(SpaceAdmissionTransportError::PeerUpgradeRequired),
                        },
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
                let decision = connection_decision(channel, error);
                self.record_connection_failure(report, channel, aggregate, commit_token, error)
                    .await;
                return (JoinerReplyHandlingOutcome::NoImmediateWork, Some(decision));
            }
        };
        let loaded = match Box::pin(self.persist_authenticated_channel(
            joiner,
            observation_material,
            channel,
            aggregate,
            commit_token,
            exchange.as_mut(),
            report,
        ))
        .await
        {
            AuthenticatedChannelProgress::Ready(loaded) => loaded,
            AuthenticatedChannelProgress::Stopped(decision) => {
                return (JoinerReplyHandlingOutcome::NoImmediateWork, decision);
            }
        };

        Box::pin(self.exchange_pending_request(
            joiner,
            observation_material,
            loaded,
            exchange,
            report,
        ))
        .await
    }

    async fn persist_authenticated_channel(
        &self,
        joiner: &JoinerAdmissionService,
        observation_material: [u8; 32],
        channel: RecoveryChannel,
        aggregate: JoinerAdmission,
        commit_token: AdmissionRecoveryCommitToken,
        exchange: &mut dyn AuthenticatedAdmissionExchangePort,
        report: &mut AdmissionRecoveryReport,
    ) -> AuthenticatedChannelProgress {
        if matches!(channel, RecoveryChannel::Continuation) {
            return AuthenticatedChannelProgress::Ready(LoadedPendingAdmission::new(
                aggregate,
                commit_token,
            ));
        }
        let peer_binding = exchange.peer_binding();
        let continuation = match exchange.take_newly_established_continuation() {
            Some(continuation) => continuation,
            None => {
                self.save_recovery_required(
                    report,
                    aggregate,
                    commit_token,
                    AdmissionRecoveryCategory::MissingKey,
                )
                .await;
                return AuthenticatedChannelProgress::Stopped(Some(
                    RecoveryDecision::RequiresRecovery(Some(RecoveryProblem::MissingCredential)),
                ));
            }
        };
        let transition = match aggregate.with_authenticated_channel(peer_binding, continuation) {
            Ok(transition) => transition,
            Err(_) => {
                report.recovery_required_count += 1;
                return AuthenticatedChannelProgress::Stopped(None);
            }
        };
        match joiner
            .observations
            .scope(
                observation_material,
                scope_pairing_work(
                    AdmissionExchangeSide::Joiner,
                    message_action(SpaceAdmissionMessageKind::JoinRequest),
                    self.commit_recovery(commit_token, transition),
                ),
            )
            .await
        {
            Ok(loaded) => {
                report.advanced_count += 1;
                AuthenticatedChannelProgress::Ready(loaded)
            }
            Err(error) => {
                self.record_state_error(report, error);
                AuthenticatedChannelProgress::Stopped(None)
            }
        }
    }

    async fn exchange_pending_request(
        &self,
        joiner: &JoinerAdmissionService,
        observation_material: [u8; 32],
        loaded: LoadedPendingAdmission,
        exchange: Box<dyn AuthenticatedAdmissionExchangePort>,
        report: &mut AdmissionRecoveryReport,
    ) -> (JoinerReplyHandlingOutcome, Option<RecoveryDecision>) {
        let (aggregate, commit_token) = loaded.into_parts();
        let Some(pending_exchange) = aggregate.pending_exchange() else {
            report.recovery_required_count += 1;
            return (JoinerReplyHandlingOutcome::NoImmediateWork, None);
        };
        let action = message_action(pending_exchange.request_envelope().kind());
        let exchanged = joiner
            .observations
            .scope(
                observation_material,
                scope_admission_action(
                    action,
                    exchange.exchange(pending_exchange.request_envelope()),
                ),
            )
            .await;
        match exchanged {
            Ok(reply) => {
                let progress = joiner
                    .observations
                    .scope(
                        observation_material,
                        scope_pairing_work(
                            AdmissionExchangeSide::Joiner,
                            action,
                            self.commit_joiner_reply_observed(
                                joiner,
                                report,
                                aggregate,
                                commit_token,
                                reply,
                            ),
                        ),
                    )
                    .await;
                (progress, None)
            }
            Err(SpaceAdmissionTransportError::PeerUpgradeRequired) => {
                self.save_peer_upgrade_result(report, aggregate, commit_token)
                    .await;
                (
                    JoinerReplyHandlingOutcome::NoImmediateWork,
                    Some(RecoveryDecision::Rejected(Some(
                        RejectionCause::PeerUpgradeRequired,
                    ))),
                )
            }
            Err(error) => {
                report.deferred_count += 1;
                (
                    JoinerReplyHandlingOutcome::NoImmediateWork,
                    Some(RecoveryDecision::Deferred(Some(
                        RecoveryDeferral::Exchange(exchange_failure(error)),
                    ))),
                )
            }
        }
    }

    async fn commit_joiner_reply_observed(
        &self,
        joiner: &JoinerAdmissionService,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        commit_token: AdmissionRecoveryCommitToken,
        reply: AuthenticatedAdmissionReply,
    ) -> JoinerReplyHandlingOutcome {
        let observation = LocalWorkObservation::begin(LocalWorkStep::JoinerProcessReply);
        let before = *report;
        let progress = self
            .commit_joiner_reply(joiner, report, aggregate, commit_token, reply)
            .await;
        observation.finish(joiner_reply_work_outcome(before, *report));
        progress
    }
}
