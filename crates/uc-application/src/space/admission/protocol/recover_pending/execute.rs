use super::model::AdmissionRecoveryReport;
use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AuthenticatedAdmissionReply,
    LoadedPendingAdmission, PendingAdmissionRecoveryStateError, PrepareJoinerCandidateError,
    SpaceAdmissionTransportError,
};
use crate::space::admission::protocol::{
    AdmissionRecoveryService, JoinerAdmissionService, SpaceAdmissionProtocol,
};
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};
use uc_core::membership::{
    AdmissionPendingRecovery, AdmissionRecoveryCategory, AdmissionTransition,
    SpaceAdmissionAggregate, SpaceAdmissionEnvelopeV1, SpaceAdmissionMessageKind,
    SpaceAdmissionRejectionReason,
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
        self.execute_exclusively(self.recovery.recover_pending(&self.joiner, trigger))
            .await
    }
}

impl AdmissionRecoveryService {
    async fn recover_pending(
        &self,
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
    ) -> AdmissionRecoveryReport {
        let mut report = AdmissionRecoveryReport::default();
        let loaded = match self.state.load(trigger).await {
            Ok(loaded) => loaded,
            Err(error) => {
                record_state_error(&mut report, error);
                return report;
            }
        };

        for loaded_admission in loaded {
            let (aggregate, commit_token) = loaded_admission.into_parts();
            let Some(recovery) = aggregate.pending_recovery() else {
                continue;
            };
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
                            record_state_error(&mut report, error);
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
            match exchange.exchange(pending_exchange.request_envelope()).await {
                Ok(reply) => {
                    self.commit_joiner_reply(joiner, &mut report, aggregate, commit_token, reply)
                        .await;
                }
                Err(_) => report.deferred_count += 1,
            }
        }

        report
    }

    async fn commit_recovery(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: AdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        self.state.commit(token, transition).await
    }

    async fn record_connection_failure(
        &self,
        report: &mut AdmissionRecoveryReport,
        channel: RecoveryChannel,
        aggregate: SpaceAdmissionAggregate,
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
        aggregate: SpaceAdmissionAggregate,
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
            Err(error) => record_state_error(report, error),
        }
    }

    async fn save_recovery_required(
        &self,
        report: &mut AdmissionRecoveryReport,
        aggregate: SpaceAdmissionAggregate,
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
            Err(error) => record_state_error(report, error),
        }
    }

    async fn commit_joiner_reply(
        &self,
        joiner: &JoinerAdmissionService,
        report: &mut AdmissionRecoveryReport,
        aggregate: SpaceAdmissionAggregate,
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
                    Err(error) => record_state_error(report, error),
                }
            }
            SpaceAdmissionMessageKind::Candidate => {
                joiner
                    .handle_candidate(self, report, aggregate, token, reply, canonical_digest)
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

impl JoinerAdmissionService {
    async fn handle_candidate(
        &self,
        recovery: &AdmissionRecoveryService,
        report: &mut AdmissionRecoveryReport,
        aggregate: SpaceAdmissionAggregate,
        token: AdmissionRecoveryCommitToken,
        reply: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) {
        let prepared = match self.prepare_candidate.prepare(&reply).await {
            Ok(prepared) => prepared,
            Err(PrepareJoinerCandidateError::Unavailable) => {
                report.deferred_count += 1;
                return;
            }
            Err(PrepareJoinerCandidateError::Invalid) => {
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
                record_state_error(report, error);
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
            Err(error) => record_state_error(report, error),
        }
    }
}

fn record_state_error(
    report: &mut AdmissionRecoveryReport,
    error: PendingAdmissionRecoveryStateError,
) {
    match error {
        PendingAdmissionRecoveryStateError::RecoveryRequired => {
            report.recovery_required_count += 1;
        }
        PendingAdmissionRecoveryStateError::Locked
        | PendingAdmissionRecoveryStateError::Unavailable
        | PendingAdmissionRecoveryStateError::StateChanged => report.deferred_count += 1,
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
