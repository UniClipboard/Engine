//! 一次认证通信内的本地进度，不创建业务节点或新的关联标识。
use super::super::{
    CompletionResult, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation, DiagnosticRole,
    OperationCompletion,
};
use super::{
    admission_attempt::AdmissionAttemptRef, emit_local, millis, record::LocalEvent,
    ObservationContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::time::Instant;
use tracing::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionExchangeSide {
    Joiner,
    Sponsor,
}
impl AdmissionExchangeSide {
    pub(super) fn role(self) -> DiagnosticRole {
        match self {
            Self::Joiner => DiagnosticRole::Joiner,
            Self::Sponsor => DiagnosticRole::Sponsor,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionExchangeStep {
    PrepareRequest,
    SendRequest,
    ReceiveReply,
    ValidateReply,
    SendAcknowledgement,
    FinishSend,
    WaitPeerFinish,
    HandleRequest,
    PrepareReply,
    SendReply,
    ReceiveAcknowledgement,
    /// Joiner 初次交换：打开流、发出初次 hello、等待 `OpaqueResponse`、发出 `OpaqueFinish`。
    OpenStream,
    SendInitialHello,
    ReceiveOpaqueResponse,
    SendOpaqueFinish,
}
impl AdmissionExchangeStep {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::PrepareRequest => "prepare_request",
            Self::SendRequest => "send_request",
            Self::ReceiveReply => "receive_reply",
            Self::ValidateReply => "validate_reply",
            Self::SendAcknowledgement => "send_acknowledgement",
            Self::FinishSend => "finish_send",
            Self::WaitPeerFinish => "wait_peer_finish",
            Self::HandleRequest => "handle_request",
            Self::PrepareReply => "prepare_reply",
            Self::SendReply => "send_reply",
            Self::ReceiveAcknowledgement => "receive_acknowledgement",
            Self::OpenStream => "open_stream",
            Self::SendInitialHello => "send_initial_hello",
            Self::ReceiveOpaqueResponse => "receive_opaque_response",
            Self::SendOpaqueFinish => "send_opaque_finish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionExchangeFailure {
    TimedOut,
    ConnectionClosed,
    IoFailed,
    InvalidMessage,
    AuthenticationRejected,
    PeerUpgradeRequired,
    Internal,
    Interrupted,
}
impl AdmissionExchangeFailure {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::TimedOut => "timed_out",
            Self::ConnectionClosed => "connection_closed",
            Self::IoFailed => "io_failed",
            Self::InvalidMessage => "invalid_message",
            Self::AuthenticationRejected => "authentication_rejected",
            Self::PeerUpgradeRequired => "peer_upgrade_required",
            Self::Internal => "internal",
            Self::Interrupted => "interrupted",
        }
    }
    fn error_type(self) -> DiagnosticErrorType {
        match self {
            Self::TimedOut => DiagnosticErrorType::Timeout,
            Self::ConnectionClosed | Self::Interrupted => DiagnosticErrorType::ChannelClosed,
            Self::IoFailed => DiagnosticErrorType::StreamFailed,
            Self::InvalidMessage => DiagnosticErrorType::DecodeFailed,
            Self::AuthenticationRejected => DiagnosticErrorType::AuthenticationFailed,
            Self::PeerUpgradeRequired => DiagnosticErrorType::PeerIncompatible,
            Self::Internal => DiagnosticErrorType::Internal,
        }
    }
}

#[derive(Clone, Copy)]
pub struct AdmissionExchangeFailureDetail {
    pub(super) side: AdmissionExchangeSide,
    pub(super) step: AdmissionExchangeStep,
    pub(super) reason: AdmissionExchangeFailure,
}
impl AdmissionExchangeFailureDetail {
    pub(super) fn local_fields(self) -> (&'static str, &'static str) {
        (self.step.as_str(), self.reason.as_str())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AdmissionExchangeEvent {
    Started {
        side: AdmissionExchangeSide,
        step: AdmissionExchangeStep,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_ref: Option<AdmissionAttemptRef>,
    },
    Finished {
        side: AdmissionExchangeSide,
        step: AdmissionExchangeStep,
        duration_ms: u64,
        failure: Option<AdmissionExchangeFailure>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_ref: Option<AdmissionAttemptRef>,
    },
}
impl AdmissionExchangeEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Started { .. } => "pairing.exchange.step.started",
            Self::Finished { .. } => "pairing.exchange.step.finished",
        }
    }
    pub(super) fn level(&self) -> &'static str {
        match self {
            Self::Finished {
                failure: Some(_), ..
            } => "WARN",
            _ => "INFO",
        }
    }
    pub(super) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        let (side, step, attempt_ref) = match self {
            Self::Started {
                side,
                step,
                attempt_ref,
            }
            | Self::Finished {
                side,
                step,
                attempt_ref,
                ..
            } => (side, step, attempt_ref),
        };
        fields.insert("uc.role".into(), json!(side));
        fields.insert("step".into(), json!(step));
        if let Some(attempt_ref) = attempt_ref {
            fields.insert("attempt_ref".into(), json!(attempt_ref));
        }
        if let Self::Finished {
            duration_ms,
            failure,
            ..
        } = self
        {
            fields.insert("duration_ms".into(), json!(duration_ms));
            fields.insert(
                "uc.outcome".into(),
                json!(match failure {
                    None => "ok",
                    Some(AdmissionExchangeFailure::Interrupted) => "cancelled",
                    Some(_) => "error",
                }),
            );
            if let Some(reason) = failure {
                fields.insert("error.reason".into(), json!(reason));
            }
        }
        fields
    }
}

pub struct AdmissionExchangeObservation {
    side: AdmissionExchangeSide,
    context: ObservationContext,
    span: Span,
    started: Instant,
    active: Option<(AdmissionExchangeStep, Instant)>,
    failure: Option<AdmissionExchangeFailureDetail>,
    finished: bool,
    /// 仅记录步骤：完成记录由调用方自己写，这里不重复结算。
    steps_only: bool,
    /// 同一次准入尝试在 joiner 与 sponsor 两端共有的关联引用。
    attempt: Option<AdmissionAttemptRef>,
}
impl AdmissionExchangeObservation {
    pub fn begin(side: AdmissionExchangeSide) -> Self {
        Self {
            side,
            context: ObservationContext::capture(),
            span: Span::current(),
            started: Instant::now(),
            active: None,
            failure: None,
            finished: false,
            steps_only: false,
            attempt: None,
        }
    }
    /// 给本次交换的步骤记录附上准入尝试引用。
    pub fn with_attempt(mut self, attempt: AdmissionAttemptRef) -> Self {
        self.attempt = Some(attempt);
        self
    }
    /// 只记录步骤，不写完成记录；用于完成记录已由调用方负责的交换（初次 hello 到 `OpaqueFinish`）。
    pub fn begin_steps_only(side: AdmissionExchangeSide) -> Self {
        let mut observation = Self::begin(side);
        observation.steps_only = true;
        observation
    }
    /// 结束步骤记录；最后一步按已记录的失败结算。
    pub fn end_steps(mut self) {
        self.finished = true;
        self.end_step(self.failure.map(|detail| detail.reason));
    }
    pub fn start_step(&mut self, step: AdmissionExchangeStep) {
        self.end_step(None);
        self.active = Some((step, Instant::now()));
        self.emit(AdmissionExchangeEvent::Started {
            side: self.side,
            step,
            attempt_ref: self.attempt,
        });
    }
    pub fn fail(&mut self, reason: AdmissionExchangeFailure) {
        if self.failure.is_none() {
            if let Some((step, _)) = self.active {
                self.failure = Some(AdmissionExchangeFailureDetail {
                    side: self.side,
                    step,
                    reason,
                });
            }
        }
    }
    pub fn finish(mut self, mut completion: OperationCompletion) {
        self.finished = true;
        self.end_step(self.failure.map(|detail| detail.reason));
        if let Some(detail) = self.failure {
            if matches!(completion.result, CompletionResult::Failed(_)) {
                completion.result = CompletionResult::Failed(detail.reason.error_type());
            }
            self.span.in_scope(|| {
                super::authentication::complete_admission_exchange_failure(detail, completion)
            });
        } else {
            self.span
                .in_scope(|| super::super::complete_operation(completion));
        }
    }
    fn end_step(&mut self, failure: Option<AdmissionExchangeFailure>) {
        if let Some((step, started)) = self.active.take() {
            self.emit(AdmissionExchangeEvent::Finished {
                side: self.side,
                step,
                duration_ms: millis(started.elapsed()),
                failure,
                attempt_ref: self.attempt,
            });
        }
    }
    fn emit(&self, record: AdmissionExchangeEvent) {
        emit_local(LocalEvent::AdmissionExchange { record }, &self.context);
    }
}
impl Drop for AdmissionExchangeObservation {
    fn drop(&mut self) {
        if !self.finished {
            if self.steps_only {
                // 提前返回的错误已经记下失败原因；没有记录失败才是被取消。
                let failure = self
                    .failure
                    .map_or(AdmissionExchangeFailure::Interrupted, |detail| {
                        detail.reason
                    });
                self.end_step(Some(failure));
                return;
            }
            self.end_step(Some(AdmissionExchangeFailure::Interrupted));
            self.span.in_scope(|| {
                super::super::complete_operation(OperationCompletion::cancelled(
                    DiagnosticDomain::SpaceAdmission,
                    DiagnosticOperation::NetworkTransport,
                    self.side.role(),
                    self.started.elapsed(),
                ))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::decode_local_record;
    use super::*;

    #[test]
    fn step_decoder_rejects_free_text_unknown_fields_and_wrong_severity() {
        let valid = json!({
            "kind": "admission_exchange",
            "record": { "event": "finished", "side": "joiner", "step": "receive_reply",
                "duration_ms": 30_000, "failure": "timed_out" },
        });
        let name = "pairing.exchange.step.finished";
        assert!(decode_local_record(name, &valid.to_string(), "WARN").is_some());
        assert!(decode_local_record(name, &valid.to_string(), "INFO").is_none());
        for (field, value) in [
            ("device_name", json!("sensitive")),
            ("step", json!("sensitive")),
            ("failure", json!("sensitive")),
            ("side", json!("sensitive")),
            ("duration_ms", json!(-1)),
        ] {
            let mut invalid = valid.clone();
            invalid["record"][field] = value;
            assert!(decode_local_record(name, &invalid.to_string(), "WARN").is_none());
        }
    }
}
