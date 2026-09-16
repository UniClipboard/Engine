//! 一次认证通信内的本地进度，不创建业务节点或新的关联标识。
use super::super::{
    CompletionResult, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation, DiagnosticRole,
    OperationCompletion,
};
use super::{emit_local, millis, record::LocalEvent, ObservationContext};
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
    },
    Finished {
        side: AdmissionExchangeSide,
        step: AdmissionExchangeStep,
        duration_ms: u64,
        failure: Option<AdmissionExchangeFailure>,
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
        let (side, step) = match self {
            Self::Started { side, step } | Self::Finished { side, step, .. } => (side, step),
        };
        fields.insert("uc.role".into(), json!(side));
        fields.insert("step".into(), json!(step));
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
        }
    }
    pub fn start_step(&mut self, step: AdmissionExchangeStep) {
        self.end_step(None);
        self.active = Some((step, Instant::now()));
        self.emit(AdmissionExchangeEvent::Started {
            side: self.side,
            step,
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
