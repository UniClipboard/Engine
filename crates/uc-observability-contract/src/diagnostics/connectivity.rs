//! 连接与恢复诊断的入口：完整动作负责结果，本模块负责关联和记录生命周期。
use super::ObservationContext;
use std::time::{Duration, Instant};

mod address;
mod address_record;

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
mod network_recovery;
mod physical;
mod record;
mod source;
pub use source::{LocalDiagnosticSource, SourceCapability, SourceCollection};

pub use authentication::complete_group_update_failure;
pub use group_update::{
    GroupUpdateFailureDetail, GroupUpdatePhase, GroupUpdateReason, GroupUpdateSource,
};

pub use network_recovery::{
    DnsProbeResult, DnsProbeStage, NetworkRecoveryResult, NetworkRecoveryTrigger,
    RecoveryActionObservation,
};

pub use physical::{
    local_connection_key, local_path_key, ConnectionLifetimeObservation, NetworkPathKind,
    ObserverFailure, PathObservationKind,
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
pub struct PresenceCheckObservation {
    context: ObservationContext,
    started: Instant,
    finished: bool,
}
impl PresenceCheckObservation {
    pub fn begin() -> Self {
        Self {
            context: ObservationContext::capture(),
            started: Instant::now(),
            finished: false,
        }
    }
    pub fn finish(mut self, result: PresenceCheckResult) {
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
