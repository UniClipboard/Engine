//! 连接与恢复诊断的入口：完整动作负责结果，本模块负责关联和记录生命周期。
use super::ObservationContext;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

mod address;
mod address_record;
mod admission_attempt;
mod admission_exchange;
mod admission_network;
mod admission_response;
mod invitation_resolution;
mod local_identity;
mod local_work;
mod maintenance;
pub use admission_attempt::AdmissionAttemptRef;
pub use admission_exchange::{
    AdmissionExchangeFailure, AdmissionExchangeFailureDetail, AdmissionExchangeObservation,
    AdmissionExchangeSide, AdmissionExchangeStep,
};
pub use admission_network::{
    record_admission_network_snapshot, AdmissionNetworkPoint, AdmissionNetworkSnapshot,
};
pub use admission_response::{
    complete_admission_response_failure, AdmissionResponseFailure, ApplicationCloseClass,
};
pub use invitation_resolution::{InvitationResolution, InvitationSource, InvitationSourceOutcome};
pub use local_identity::{record_local_identity_changed, LocalIdentityState};
pub use local_work::{
    observe_blob_publish_sync_result, observe_invitation_resolution, observe_local_result,
    observe_local_sync_result, scope_blob_publish, scope_pairing_work, LocalWorkContext,
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};
pub use maintenance::{
    record_pending_group_updates, MaintenanceDisposition, MaintenanceObservation,
};

pub use address_record::{
    local_address_record_keys, AddressRecordResult, StoredAddressObservation,
};
mod authentication;
mod clipboard_receive;
pub use authentication::complete_clipboard_receive_failure;
pub use clipboard_receive::{
    describe_clipboard_receive_failure, ClipboardReceiveFailure, ClipboardReceiveObservation,
};
mod connection;
mod group_update;
mod inbound_peer;
mod membership_history;
mod network_recovery;
mod physical;
mod record;
mod source;
pub use source::{LocalDiagnosticSource, SourceCapability, SourceCollection};

pub use authentication::complete_group_update_failure;
pub use authentication::complete_membership_history_failure;
pub use group_update::{
    ClassifiedGroupUpdateStorageFailure, GroupUpdateFailureDetail, GroupUpdatePhase,
    GroupUpdateReason, GroupUpdateSource,
};
pub use inbound_peer::{
    record_inbound_peer_rejection, InboundPeerProtocol, InboundPeerRejectionReason,
};
pub use membership_history::{
    MembershipHistoryFailureDetail, MembershipHistoryFailurePhase, MembershipHistoryFailureReason,
};

pub use network_recovery::{
    DnsProbeResult, DnsProbeStage, NetworkRecoveryResult, NetworkRecoveryTrigger,
    RecoveryActionObservation,
};

pub use physical::{
    local_connection_key, local_path_key, ConnectionLifetimeObservation, NegotiatedProtocol,
    NetworkPathKind, ObserverFailure, PathObservationKind,
};

pub use address::{
    local_candidate_fingerprint, AddressInputSource, CandidateSummary, DiscoverySource,
    LookupObservation, NetworkRecorder,
};

pub use connection::{
    local_connection_peer, ConnectionAttemptObservation, ConnectionAttempts,
    ConnectionFailurePhase, ConnectionFailureReason, ConnectionObservation, ConnectionOutcome,
    ConnectionPurpose,
};

pub use authentication::{
    complete_admission_authentication_failure, complete_admission_connection_failure,
    take_local_completion_detail, AuthenticationFailure, AuthenticationStage, CredentialFailure,
    IdentityCheck, LocalCompletionDetail, ProofFailure, ReadFailure,
};
use record::LocalEvent;
pub use record::{
    decode_local_record, ConfirmationFailure, ConnectionCloseReason, ConnectionDirection,
    DialFailure, ExchangeFailure, PresenceCheckResult, RecoveryDecision, RecoveryDeferral,
    RecoveryProblem, RecoveryTrigger, RejectionCause, SessionFailure, SessionTransition,
    SessionTransitionResult, StateFailure,
};

pub const CONNECTIVITY_TARGET: &str = "uc.connectivity";

/// 确认真正需要切换后报告已测量的锁等待；空轮询不产生记录，不伪造开始事件。
pub fn record_session_lock_wait(waited: Duration) {
    emit_local(
        LocalEvent::SessionLockWait {
            duration_ms: millis(waited),
        },
        &ObservationContext::capture(),
    );
}

fn local_events_enabled() -> bool {
    tracing::event_enabled!(target: "uc.connectivity", tracing::Level::INFO)
}

pub fn record_admission_recovery_decision(
    context: &ObservationContext,
    trigger: RecoveryTrigger,
    decision: RecoveryDecision,
) {
    emit_local(LocalEvent::Recovery { trigger, decision }, context);
}
pub fn record_presence_closed(direction: ConnectionDirection, reason: ConnectionCloseReason) {
    // 关闭观察由长期任务发出，不继承已经结束的短期拨号上下文。
    let independent = ObservationContext(opentelemetry::Context::new());
    emit_local(
        LocalEvent::PresenceClosed { direction, reason },
        &independent,
    );
}

pub struct SessionTransitionObservation {
    transition: SessionTransition,
    context: ObservationContext,
    started: Instant,
    finished: bool,
}
impl SessionTransitionObservation {
    pub fn begin(transition: SessionTransition) -> Self {
        let observation = Self {
            transition,
            context: ObservationContext::capture(),
            started: Instant::now(),
            finished: false,
        };
        emit_local(
            LocalEvent::SessionStarted { transition },
            &observation.context,
        );
        observation
    }
    pub fn finish(mut self, result: SessionTransitionResult) {
        self.finished = true;
        emit_local(
            LocalEvent::SessionFinished {
                transition: self.transition,
                result,
                duration_ms: millis(self.started.elapsed()),
            },
            &self.context,
        );
    }
}
impl Drop for SessionTransitionObservation {
    fn drop(&mut self) {
        if !self.finished {
            emit_local(
                LocalEvent::SessionFinished {
                    transition: self.transition,
                    result: SessionTransitionResult::Interrupted,
                    duration_ms: millis(self.started.elapsed()),
                },
                &self.context,
            );
        }
    }
}
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn emit_local(event: LocalEvent, context: &ObservationContext) {
    let Ok(payload) = serde_json::to_string(&event) else {
        return;
    };
    let _guard = context.0.clone().attach();
    let name = event.name();
    match event.level() {
        "WARN" => {
            tracing::event!(target: "uc.connectivity", tracing::Level::WARN, event.name = name, payload = payload.as_str())
        }
        "ERROR" => {
            tracing::event!(target: "uc.connectivity", tracing::Level::ERROR, event.name = name, payload = payload.as_str())
        }
        _ => {
            tracing::event!(target: "uc.connectivity", tracing::Level::INFO, event.name = name, payload = payload.as_str())
        }
    }
}

/// 在线检查只在完整返回或被取消时结算，具体分支不各自输出日志。

/// 完整 owner 只能提交固定结局；不提供业务状态或阶段查询。
#[derive(Clone, Default)]
pub struct PresenceCheckFeedback(Option<Arc<Mutex<Option<PresenceCheckResult>>>>);
tokio::task_local! { static PRESENCE_FEEDBACK: PresenceCheckFeedback; }
impl PresenceCheckFeedback {
    pub fn capture() -> Self {
        PRESENCE_FEEDBACK.try_with(Clone::clone).unwrap_or_default()
    }
    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        PRESENCE_FEEDBACK.scope(self, future).await
    }
}
pub fn record_presence_check_result(result: PresenceCheckResult) {
    let _ = PRESENCE_FEEDBACK.try_with(|feedback| {
        if let Some(value) = &feedback.0 {
            *value.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        }
    });
}
pub struct PresenceCheckObservation {
    feedback: PresenceCheckFeedback,
    context: ObservationContext,
    started: Instant,
    finished: bool,
}
impl PresenceCheckObservation {
    pub fn begin() -> Self {
        Self {
            feedback: PresenceCheckFeedback(Some(Arc::new(Mutex::new(None)))),
            context: ObservationContext::capture(),
            started: Instant::now(),
            finished: false,
        }
    }
    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        self.feedback.clone().scope(future).await
    }
    pub fn finish(mut self, result: PresenceCheckResult) {
        let result = self
            .feedback
            .0
            .as_ref()
            .and_then(|value| *value.lock().unwrap_or_else(PoisonError::into_inner))
            .unwrap_or(result);
        self.finished = true;
        emit_local(
            LocalEvent::PresenceCheck {
                result,
                duration_ms: millis(self.started.elapsed()),
            },
            &self.context,
        );
    }
}
impl Drop for PresenceCheckObservation {
    fn drop(&mut self) {
        if !self.finished {
            emit_local(
                LocalEvent::PresenceCheck {
                    result: PresenceCheckResult::Interrupted,
                    duration_ms: millis(self.started.elapsed()),
                },
                &self.context,
            );
        }
    }
}
