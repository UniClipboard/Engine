use super::model::{AdmissionRecoveryDisposition, AdmissionRecoveryReport};
use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AuthenticatedAdmissionReply,
    LoadedPendingAdmission, PendingAdmissionRecoveryStateError, SpaceAdmissionTransportError,
};
use crate::space::admission::protocol::{
    AdmissionRecoveryService, ExecuteJoinerActivationError, JoinerAdmissionService,
    JoinerReplyHandlingOutcome, SpaceAdmissionProtocol,
};
use crate::space::membership::{
    AdmissionAbandonmentRevocationTarget, AdmissionMaintenanceOutcome, AdmissionRevocationTarget,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
    RemoveSpaceMemberError,
};
use uc_core::membership::{
    AdmissionRecoveryCategory, JoinerAdmission, SpaceAdmissionMessageKind,
    SpaceAdmissionRejectionReason, SponsorAbandonmentCleanup, SponsorPairingConfirmationStatus,
};
use uc_observability_contract::diagnostics::connectivity::{
    record_admission_recovery_decision, ExchangeFailure, LocalWorkObservation, LocalWorkOutcome,
    LocalWorkStep, RecoveryDecision, RecoveryDeferral, RecoveryProblem, RecoveryTrigger,
    RejectionCause, StateFailure,
};
use uc_observability_contract::diagnostics::{
    DiagnosticErrorType, ObservationContext, SpaceAdmissionObservationOutcome,
};

mod joiner_network;

#[derive(Clone, Copy)]
enum RecoveryChannel {
    Initial,
    Continuation,
}

enum AuthenticatedChannelProgress {
    Ready(LoadedPendingAdmission),
    Stopped(Option<RecoveryDecision>),
}

struct JoinerRecoveryObservation {
    trigger: AdmissionRecoveryTrigger,
    material: [u8; 32],
    was_cancelling: bool,
    before: AdmissionRecoveryReport,
    context: ObservationContext,
}

impl JoinerRecoveryObservation {
    async fn begin(
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
        aggregate: &JoinerAdmission,
        before: AdmissionRecoveryReport,
    ) -> Self {
        let material = *aggregate.admission_id().as_bytes();
        let context = joiner
            .observations
            .scope(material, async { ObservationContext::capture() })
            .await;
        Self {
            trigger,
            material,
            was_cancelling: aggregate.is_cancelling() || aggregate.termination_reason().is_some(),
            before,
            context,
        }
    }

    fn finish(
        self,
        joiner: &JoinerAdmissionService,
        after: AdmissionRecoveryReport,
        hint: Option<RecoveryDecision>,
    ) {
        if let Some(decision) =
            actual_recovery_decision(self.before, after, self.was_cancelling, hint)
        {
            record_admission_recovery_decision(
                &self.context,
                diagnostic_trigger(self.trigger),
                decision,
            );
        }
        finish_observation_after_recovery(
            joiner,
            self.material,
            self.was_cancelling,
            self.before,
            after,
        );
    }
}

// 完整协议最多包含加入、准备、应用和最终确认四次请求。
// 上限只防止异常对端让同一轮无限推进，不参与正常重试计时。
const MAX_IMMEDIATE_EXCHANGES_PER_ADMISSION: usize = 4;

impl SpaceAdmissionProtocol {
    pub(crate) async fn recover_pending(
        &self,
        trigger: AdmissionRecoveryTrigger,
    ) -> AdmissionRecoveryReport {
        Box::pin(self.recovery.recover_pending(&self.joiner, trigger)).await
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
        let waiting = LocalWorkObservation::begin(LocalWorkStep::RecoveryLock);
        let _recovery = self.execution_lock.lock().await;
        waiting.finish(LocalWorkOutcome::Ok);
        loop {
            // 本机取消或替换保存后，丢弃旧状态仍在等待的网络动作，
            // 并由同一个恢复 owner 立即重新读取当前状态继续处理。
            let mut interrupted = self.interrupt_generation.subscribe();
            tokio::select! {
                biased;
                changed = interrupted.changed() => {
                    if changed.is_err() {
                        return Box::pin(self.recover_pending_once(joiner, trigger)).await;
                    }
                }
                report = Box::pin(self.recover_pending_once(joiner, trigger)) => return report,
            }
        }
    }

    async fn recover_pending_once(
        &self,
        joiner: &JoinerAdmissionService,
        trigger: AdmissionRecoveryTrigger,
    ) -> AdmissionRecoveryReport {
        let mut report = AdmissionRecoveryReport::default();
        let now_ms = self.clock.now_ms();
        let recovery = match self.state.load(trigger, now_ms).await {
            Ok(loaded) => loaded,
            Err(error) => {
                record_recovery_load_error(joiner, trigger, &error);
                self.record_state_error(&mut report, error);
                return report;
            }
        };
        let (
            loaded,
            sponsor_deadlines,
            sponsor_abandonments,
            next_deadline_ms,
            sponsor_confirmation_pending,
        ) = recovery.into_parts();
        if sponsor_confirmation_pending {
            report.disposition = AdmissionRecoveryDisposition::YieldMaintenance;
        }
        if let Some(deadline_ms) = next_deadline_ms {
            joiner.maintenance_wake.schedule_at(deadline_ms, now_ms);
        }
        for loaded in sponsor_deadlines {
            let (aggregate, token) = loaded.into_parts();
            let awaiting_confirmation = aggregate.pairing_confirmation().is_some_and(|summary| {
                summary.status() == SponsorPairingConfirmationStatus::AwaitingPeerConfirmation
            });
            let transition = if awaiting_confirmation {
                aggregate.mark_confirmation_unconfirmed(now_ms)
            } else {
                aggregate.terminate_if_expired(now_ms)
            };
            match transition {
                Ok(Some(transition)) => {
                    match self
                        .commit_sponsor_deadline_and_notify(token, transition)
                        .await
                    {
                        Ok(_) => report.advanced_count += 1,
                        Err(error) => self.record_state_error(&mut report, error),
                    }
                }
                Ok(None) => {}
                Err(_) => report.recovery_required_count += 1,
            }
        }
        let mut joiner_network = Vec::new();
        for loaded in loaded {
            if let Some(loaded) = self
                .recover_joiner_local_work(joiner, loaded, &mut report)
                .await
            {
                joiner_network.push(loaded);
            }
        }
        for loaded in sponsor_abandonments {
            let (aggregate, token) = loaded.into_parts();
            let revocation = match aggregate.abandonment_cleanup() {
                Some(SponsorAbandonmentCleanup::Known(binding)) => {
                    self.admission_revocation
                        .revoke_admission(AdmissionRevocationTarget::new(
                            aggregate.admission_id(),
                            binding.clone(),
                        ))
                        .await
                }
                Some(SponsorAbandonmentCleanup::Unknown {
                    attempt_digest,
                    member_instance_id,
                    add_event_id,
                }) => {
                    self.admission_revocation
                        .revoke_abandoned_admission(AdmissionAbandonmentRevocationTarget::new(
                            aggregate.admission_id(),
                            *attempt_digest,
                            *member_instance_id,
                            *add_event_id,
                        ))
                        .await
                }
                Some(SponsorAbandonmentCleanup::NotRequired) | None => continue,
            };
            match revocation {
                // 原成员已经不存在、或本机已经失去成员资格时，没有可继续执行的撤销动作。
                // 终止记录仍然完成，不能把设备永久卡在恢复流程里。
                Ok(_)
                | Err(
                    RemoveSpaceMemberError::CommittedButPending { .. }
                    | RemoveSpaceMemberError::LocalMemberRemoved
                    | RemoveSpaceMemberError::TargetNotFound,
                ) => match aggregate.complete_abandonment_cleanup() {
                    Ok(transition) => match self
                        .commit_sponsor_abandonment_and_notify(token, transition)
                        .await
                    {
                        Ok(_) => report.advanced_count += 1,
                        Err(error) => self.record_state_error(&mut report, error),
                    },
                    Err(_) => report.recovery_required_count += 1,
                },
                Err(
                    RemoveSpaceMemberError::Locked
                    | RemoveSpaceMemberError::StateChanged
                    | RemoveSpaceMemberError::Unavailable,
                ) => report.deferred_count += 1,
                Err(
                    RemoveSpaceMemberError::RecoveryRequired | RemoveSpaceMemberError::SelfTarget,
                ) => report.recovery_required_count += 1,
            }
        }
        Box::pin(self.recover_joiner_network_work(joiner, trigger, joiner_network, &mut report))
            .await;

        report
    }

    async fn recover_joiner_local_work(
        &self,
        joiner: &JoinerAdmissionService,
        loaded: LoadedPendingAdmission,
        report: &mut AdmissionRecoveryReport,
    ) -> Option<LoadedPendingAdmission> {
        let (aggregate, commit_token) = loaded.into_parts();
        let now_ms = self.clock.now_ms();
        let loaded =
            recover_local_termination(self, joiner, aggregate, commit_token, report).await?;
        let (aggregate, commit_token) = loaded.into_parts();
        if aggregate.is_expired_at(now_ms) != Some(true) || !aggregate.can_terminate_locally() {
            return Some(LoadedPendingAdmission::new(aggregate, commit_token));
        }
        let observation_material = *aggregate.admission_id().as_bytes();
        let transition = match aggregate.terminate_if_expired(now_ms) {
            Ok(Some(transition)) => transition,
            Ok(None) => return None,
            Err(_) => {
                report.recovery_required_count += 1;
                return None;
            }
        };
        match self
            .commit_recovery_and_notify(commit_token, transition)
            .await
        {
            Ok(loaded) => {
                report.terminated_count += 1;
                let (terminated, token) = loaded.into_parts();
                recover_local_termination(self, joiner, terminated, token, report).await?;
                joiner.observations.finish(
                    observation_material,
                    SpaceAdmissionObservationOutcome::Failed(DiagnosticErrorType::Timeout),
                );
            }
            Err(error) => self.record_state_error(report, error),
        }
        None
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
    ) -> JoinerReplyHandlingOutcome {
        let (reply, canonical_digest) = reply.into_parts();
        match reply.kind() {
            SpaceAdmissionMessageKind::Rejected => {
                let transition = match aggregate.accept_rejection(reply, canonical_digest) {
                    Ok(transition) => transition,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return JoinerReplyHandlingOutcome::NoImmediateWork;
                    }
                };
                match self.commit_recovery_and_notify(token, transition).await {
                    Ok(_) => report.rejected_count += 1,
                    Err(error) => self.record_state_error(report, error),
                }
                JoinerReplyHandlingOutcome::NoImmediateWork
            }
            SpaceAdmissionMessageKind::Candidate => {
                joiner
                    .handle_candidate(self, report, aggregate, token, reply, canonical_digest)
                    .await
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
                    .await
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
                    .await
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
                    .await
            }
            SpaceAdmissionMessageKind::Abandoned => {
                let transition = match aggregate.accept_abandoned(reply, canonical_digest) {
                    Ok(transition) => transition,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return JoinerReplyHandlingOutcome::NoImmediateWork;
                    }
                };
                match self.commit_recovery(token, transition).await {
                    Ok(_) => report.advanced_count += 1,
                    Err(error) => self.record_state_error(report, error),
                }
                JoinerReplyHandlingOutcome::NoImmediateWork
            }
            _ => {
                self.save_recovery_required(
                    report,
                    aggregate,
                    token,
                    AdmissionRecoveryCategory::ProtocolConflict,
                )
                .await;
                JoinerReplyHandlingOutcome::NoImmediateWork
            }
        }
    }
}

fn record_recovery_load_error(
    joiner: &JoinerAdmissionService,
    trigger: AdmissionRecoveryTrigger,
    error: &PendingAdmissionRecoveryStateError,
) {
    let decision = match error {
        PendingAdmissionRecoveryStateError::RecoveryRequired => {
            RecoveryDecision::RequiresRecovery(Some(RecoveryProblem::CorruptState))
        }
        PendingAdmissionRecoveryStateError::Locked => {
            RecoveryDecision::Deferred(Some(RecoveryDeferral::State(StateFailure::Locked)))
        }
        PendingAdmissionRecoveryStateError::Unavailable => {
            RecoveryDecision::Deferred(Some(RecoveryDeferral::State(StateFailure::Unavailable)))
        }
        PendingAdmissionRecoveryStateError::StateChanged => {
            RecoveryDecision::Deferred(Some(RecoveryDeferral::State(StateFailure::Changed)))
        }
    };
    record_admission_recovery_decision(
        &ObservationContext::capture(),
        diagnostic_trigger(trigger),
        decision,
    );
    let outcome = match error {
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
}

async fn recover_local_termination(
    recovery: &AdmissionRecoveryService,
    joiner: &JoinerAdmissionService,
    aggregate: JoinerAdmission,
    token: AdmissionRecoveryCommitToken,
    report: &mut AdmissionRecoveryReport,
) -> Option<LoadedPendingAdmission> {
    let Some(transition_bytes) = aggregate
        .cleanup_obligation()
        .and_then(|cleanup| cleanup.local_space_transition())
        .map(|transition| transition.as_bytes().to_vec())
    else {
        return Some(LoadedPendingAdmission::new(aggregate, token));
    };
    match joiner
        .execute_activation
        .terminate(aggregate.admission_id(), &transition_bytes)
        .await
    {
        Ok(()) => {
            let transition = match aggregate.complete_local_space_termination() {
                Ok(transition) => transition,
                Err(_) => {
                    report.recovery_required_count += 1;
                    return None;
                }
            };
            match recovery.commit_recovery_and_notify(token, transition).await {
                Ok(loaded) => {
                    report.advanced_count += 1;
                    Some(loaded)
                }
                Err(error) => {
                    recovery.record_state_error(report, error);
                    None
                }
            }
        }
        Err(ExecuteJoinerActivationError::Unavailable { .. }) => {
            report.deferred_count += 1;
            None
        }
        Err(ExecuteJoinerActivationError::Invalid { .. }) => {
            report.recovery_required_count += 1;
            None
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
    } else if was_cancelling && after.advanced_count > before.advanced_count {
        Some(SpaceAdmissionObservationOutcome::Cancelled)
    } else {
        None
    };
    if let Some(outcome) = outcome {
        joiner.observations.finish(material, outcome);
    }
}

fn joiner_reply_work_outcome(
    before: AdmissionRecoveryReport,
    after: AdmissionRecoveryReport,
) -> LocalWorkOutcome {
    if after.recovery_required_count > before.recovery_required_count {
        LocalWorkOutcome::Corrupt
    } else if after.deferred_count > before.deferred_count {
        LocalWorkOutcome::Deferred
    } else if after.rejected_count > before.rejected_count
        || after.peer_upgrade_required_count > before.peer_upgrade_required_count
    {
        LocalWorkOutcome::Rejected
    } else {
        LocalWorkOutcome::Ok
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
    if cancelling && after.advanced_count > before.advanced_count {
        return Some(RecoveryDecision::Cancelled);
    }
    None
}

#[async_trait::async_trait]
impl RecoverSpaceAdmissionsPort for SpaceAdmissionProtocol {
    async fn recover_space_admissions(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome {
        let trigger = match trigger {
            MembershipMaintenanceTrigger::PeerContact(_) => {
                return AdmissionMaintenanceOutcome::Continue(
                    MembershipMaintenanceStepOutcome::Completed,
                );
            }
            MembershipMaintenanceTrigger::Startup => AdmissionRecoveryTrigger::Startup,
            MembershipMaintenanceTrigger::Resume => AdmissionRecoveryTrigger::Resume,
            MembershipMaintenanceTrigger::Periodic => AdmissionRecoveryTrigger::Periodic,
            MembershipMaintenanceTrigger::StateChanged => AdmissionRecoveryTrigger::StateChanged,
            MembershipMaintenanceTrigger::PeerOnline(device_id) => {
                AdmissionRecoveryTrigger::PeerOnline(*device_id)
            }
        };
        let report = self.recover_pending(trigger).await;
        let outcome = if report.recovery_required_count > 0 {
            MembershipMaintenanceStepOutcome::Corrupt
        } else if report.peer_upgrade_required_count > 0 || report.rejected_count > 0 {
            MembershipMaintenanceStepOutcome::StableFailure
        } else if report.deferred_count > 0 {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            MembershipMaintenanceStepOutcome::Completed
        };
        match report.disposition {
            AdmissionRecoveryDisposition::ContinueMaintenance => {
                AdmissionMaintenanceOutcome::Continue(outcome)
            }
            AdmissionRecoveryDisposition::YieldMaintenance => {
                AdmissionMaintenanceOutcome::Yield(outcome)
            }
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
