use super::model::AdmissionRecoveryReport;
use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AuthenticatedAdmissionReply,
    LoadedPendingAdmission, SpaceAdmissionTransportError,
};
use crate::space::admission::protocol::{
    AdmissionRecoveryService, JoinerAdmissionService, SpaceAdmissionProtocol,
};
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};
use std::time::Instant;
use uc_core::membership::{
    AdmissionPendingRecovery, AdmissionRecoveryCategory, JoinerAdmission,
    SpaceAdmissionMessageKind, SpaceAdmissionRejectionReason,
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
        self.execute_exclusively(async {
            self.recovery.recover_pending(&self.joiner, trigger).await
        })
        .await
    }
}

impl AdmissionRecoveryService {
    async fn recover_pending(
        &self,
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
    ) -> AdmissionRecoveryReport {
        let round_started = Instant::now();
        let mut report = AdmissionRecoveryReport::default();
        let load_started = Instant::now();
        let loaded_result = self.state.load(trigger).await;
        if match &loaded_result {
            Ok(loaded) => !loaded.is_empty(),
            Err(_) => true,
        } {
            crate::space::admission::protocol::record_performance_phase(
                "joiner_state_load",
                load_started,
                loaded_result.is_ok(),
            );
        }
        let loaded = match loaded_result {
            Ok(loaded) => loaded,
            Err(error) => {
                self.record_state_error(&mut report, error);
                return report;
            }
        };

        for loaded_admission in loaded {
            let (aggregate, commit_token) = loaded_admission.into_parts();
            if aggregate.invitation_resolution().is_some() {
                joiner
                    .recover_invitation_resolution(self, &mut report, aggregate, commit_token)
                    .await;
                continue;
            }
            let Some(recovery) = aggregate.pending_recovery() else {
                continue;
            };
            let connection_started = Instant::now();
            let (channel_kind, established) = match recovery {
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
            };
            tracing::info!(
                target: "admission.performance",
                phase = "joiner_channel_establish",
                channel = channel_kind.as_str(),
                elapsed_ms = connection_started.elapsed().as_millis() as u64,
                outcome = if established.is_ok() { "ok" } else { "error" },
                "pairing phase completed"
            );

            let mut exchange = match established {
                Ok(exchange) => exchange,
                Err(error) => {
                    self.record_connection_failure(
                        &mut report,
                        channel_kind,
                        aggregate,
                        commit_token,
                        error,
                    )
                    .await;
                    continue;
                }
            };

            let loaded = match channel_kind {
                RecoveryChannel::Initial => {
                    let peer_binding = exchange.peer_binding();
                    let Some(continuation) = exchange.take_newly_established_continuation() else {
                        self.save_recovery_required(
                            &mut report,
                            aggregate,
                            commit_token,
                            AdmissionRecoveryCategory::MissingKey,
                        )
                        .await;
                        continue;
                    };
                    let transition =
                        match aggregate.with_authenticated_channel(peer_binding, continuation) {
                            Ok(transition) => transition,
                            Err(_) => {
                                report.recovery_required_count += 1;
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
                continue;
            };
            let exchange_started = Instant::now();
            let exchanged = exchange.exchange(pending_exchange.request_envelope()).await;
            tracing::info!(
                target: "admission.performance",
                phase = "joiner_message_exchange",
                message_kind = ?pending_exchange.request_envelope().kind(),
                elapsed_ms = exchange_started.elapsed().as_millis() as u64,
                outcome = if exchanged.is_ok() { "ok" } else { "error" },
                "pairing phase completed"
            );
            match exchanged {
                Ok(reply) => {
                    self.commit_joiner_reply(joiner, &mut report, aggregate, commit_token, reply)
                        .await;
                }
                Err(_) => report.deferred_count += 1,
            }
        }

        if report.advanced_count > 0
            || report.deferred_count > 0
            || report.rejected_count > 0
            || report.recovery_required_count > 0
        {
            tracing::info!(
                target: "admission.performance",
                phase = "joiner_recovery_round",
                elapsed_ms = round_started.elapsed().as_millis() as u64,
                advanced_count = report.advanced_count,
                deferred_count = report.deferred_count,
                rejected_count = report.rejected_count,
                recovery_required_count = report.recovery_required_count,
                "pairing recovery round completed"
            );
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
                self.save_initial_rejection(
                    report,
                    aggregate,
                    token,
                    SpaceAdmissionRejectionReason::PeerUpgradeRequired,
                )
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
        match self.commit_recovery(token, transition).await {
            Ok(_) => report.rejected_count += 1,
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
                match self.commit_recovery(token, transition).await {
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
                joiner
                    .handle_commit(self, report, aggregate, token, reply, canonical_digest)
                    .await;
            }
            SpaceAdmissionMessageKind::Complete => {
                joiner
                    .handle_complete(self, report, aggregate, token, reply, canonical_digest)
                    .await;
            }
            SpaceAdmissionMessageKind::Settled => {
                joiner
                    .handle_settled(self, report, aggregate, token, reply, canonical_digest)
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

impl RecoveryChannel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Continuation => "continuation",
        }
    }
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
        } else if report.rejected_count > 0 {
            MembershipMaintenanceStepOutcome::StableFailure
        } else if report.deferred_count > 0 {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            MembershipMaintenanceStepOutcome::Completed
        }
    }
}
