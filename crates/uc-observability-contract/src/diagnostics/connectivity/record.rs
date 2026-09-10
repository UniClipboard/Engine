//! 独立动作记录的封闭数据模型、字段编码与校验共用同一事实来源。
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryTrigger {
    Startup,
    Resume,
    Periodic,
    StateChanged,
    PeerOnline,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialFailure {
    TimedOut,
    TransportFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeFailure {
    Deferred,
    Unavailable,
    AuthenticationRejected,
    ProtocolRejected,
    InvitationUnavailable,
    PeerUpgradeRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateFailure {
    Locked,
    Unavailable,
    Changed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryDeferral {
    Connect(ExchangeFailure),
    Exchange(ExchangeFailure),
    State(StateFailure),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCause {
    InvitationUnavailable,
    AuthenticationRejected,
    PeerUpgradeRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProblem {
    CorruptState,
    ProtocolConflict,
    MissingCredential,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryDecision {
    Deferred(Option<RecoveryDeferral>),
    Rejected(Option<RejectionCause>),
    RequiresRecovery(Option<RecoveryProblem>),
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationFailure {
    TimedOut,
    TransportFailed,
    PeerNotAdmitted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PresenceCheckResult {
    Reachable,
    AddressMissing,
    AddressUnavailable,
    Dial(DialFailure),
    Confirmation(ConfirmationFailure),
    Interrupted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionDirection {
    Inbound,
    Outbound,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionCloseReason {
    LocalClosed,
    RemoteApplicationClosed,
    RemoteTransportClosed,
    RemoteReset,
    TimedOut,
    VersionMismatch,
    TransportFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTransition {
    Suspend,
    Resume,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTransitionResult {
    Completed,
    Skipped,
    Failed(SessionFailure),
    Interrupted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionFailure {
    InvalidInput,
    InvalidState,
    Unauthorized,
    NotFound,
    Conflict,
    Unavailable,
    DeadlineExceeded,
    Internal,
}

// 内部传递的是这个封闭结构的 JSON 编码，不接受调用方提供的正文或任意属性。
// 序列化与反序列化共用同一个类型，运行时必须解码成功后才展开写入文件。
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum LocalEvent {
    Recovery {
        trigger: RecoveryTrigger,
        decision: RecoveryDecision,
    },
    PresenceCheck {
        result: PresenceCheckResult,
        duration_ms: u64,
    },
    PresenceClosed {
        direction: ConnectionDirection,
        reason: ConnectionCloseReason,
    },
    SessionStarted {
        transition: SessionTransition,
    },
    SessionFinished {
        transition: SessionTransition,
        result: SessionTransitionResult,
        duration_ms: u64,
    },
}

impl LocalEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Recovery { .. } => "pairing.recovery.decided",
            Self::PresenceCheck { .. } => "presence.check.completed",
            Self::PresenceClosed { .. } => "presence.connection.closed",
            Self::SessionStarted { .. } => "session.transition.started",
            Self::SessionFinished { .. } => "session.transition.finished",
        }
    }
    pub(super) fn level(&self) -> &'static str {
        match self {
            Self::Recovery {
                decision: RecoveryDecision::RequiresRecovery(_),
                ..
            }
            | Self::SessionFinished {
                result: SessionTransitionResult::Failed(SessionFailure::Internal),
                ..
            } => "ERROR",
            Self::Recovery {
                decision: RecoveryDecision::Rejected(_),
                ..
            }
            | Self::Recovery {
                decision:
                    RecoveryDecision::Deferred(Some(RecoveryDeferral::Exchange(
                        ExchangeFailure::AuthenticationRejected | ExchangeFailure::ProtocolRejected,
                    ))),
                ..
            }
            | Self::PresenceCheck {
                result:
                    PresenceCheckResult::Confirmation(ConfirmationFailure::PeerNotAdmitted)
                    | PresenceCheckResult::AddressUnavailable,
                ..
            }
            | Self::SessionFinished {
                result: SessionTransitionResult::Failed(_) | SessionTransitionResult::Interrupted,
                ..
            } => "WARN",
            _ => "INFO",
        }
    }
    fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("event.name".into(), json!(self.name()));
        match self {
            Self::Recovery { trigger, decision } => {
                fields.insert("trigger".into(), json!(trigger));
                let (outcome, next, failure) = match decision {
                    RecoveryDecision::Deferred(cause) => (
                        "deferred",
                        "wait_for_recovery_trigger",
                        cause.map(|cause| match cause {
                            RecoveryDeferral::Connect(reason) => {
                                ("connect", exchange_reason(reason))
                            }
                            RecoveryDeferral::Exchange(reason) => {
                                ("exchange", exchange_reason(reason))
                            }
                            RecoveryDeferral::State(reason) => (
                                "state",
                                match reason {
                                    StateFailure::Locked => "storage_locked",
                                    StateFailure::Unavailable => "storage_unavailable",
                                    StateFailure::Changed => "state_changed",
                                },
                            ),
                        }),
                    ),
                    RecoveryDecision::Rejected(cause) => (
                        "rejected",
                        "none",
                        cause.map(|cause| {
                            (
                                "connect",
                                match cause {
                                    RejectionCause::InvitationUnavailable => {
                                        "invitation_unavailable"
                                    }
                                    RejectionCause::AuthenticationRejected => {
                                        "authentication_rejected"
                                    }
                                    RejectionCause::PeerUpgradeRequired => "peer_upgrade_required",
                                },
                            )
                        }),
                    ),
                    RecoveryDecision::RequiresRecovery(cause) => (
                        "error",
                        "recovery_required",
                        cause.map(|cause| {
                            (
                                "state",
                                match cause {
                                    RecoveryProblem::CorruptState => "storage_corrupt",
                                    RecoveryProblem::ProtocolConflict => "protocol_conflict",
                                    RecoveryProblem::MissingCredential => "credential_missing",
                                },
                            )
                        }),
                    ),
                    RecoveryDecision::Cancelled => ("cancelled", "none", None),
                };
                fields.insert("uc.outcome".into(), json!(outcome));
                fields.insert("next_action".into(), json!(next));
                insert_failure(&mut fields, failure);
            }
            Self::PresenceCheck {
                result,
                duration_ms,
            } => {
                let (outcome, failure) = match result {
                    PresenceCheckResult::Reachable => ("ok", None),
                    PresenceCheckResult::AddressMissing => {
                        ("deferred", Some(("address", "address_missing")))
                    }
                    PresenceCheckResult::AddressUnavailable => {
                        ("error", Some(("address", "address_unavailable")))
                    }
                    PresenceCheckResult::Dial(reason) => {
                        ("deferred", Some(("connect", dial_reason(*reason))))
                    }
                    PresenceCheckResult::Confirmation(reason) => (
                        if *reason == ConfirmationFailure::PeerNotAdmitted {
                            "rejected"
                        } else {
                            "deferred"
                        },
                        Some((
                            "confirmation",
                            match reason {
                                ConfirmationFailure::TimedOut => "timed_out",
                                ConfirmationFailure::TransportFailed => "transport_failed",
                                ConfirmationFailure::PeerNotAdmitted => "peer_not_admitted",
                            },
                        )),
                    ),
                    PresenceCheckResult::Interrupted => ("cancelled", None),
                };
                fields.insert("uc.outcome".into(), json!(outcome));
                fields.insert("duration_ms".into(), json!(duration_ms));
                insert_failure(&mut fields, failure);
            }
            Self::PresenceClosed { direction, reason } => {
                fields.insert("direction".into(), json!(direction));
                fields.insert("close.reason".into(), json!(reason));
            }
            Self::SessionStarted { transition } => {
                fields.insert("transition".into(), json!(transition));
            }
            Self::SessionFinished {
                transition,
                result,
                duration_ms,
            } => {
                fields.insert("transition".into(), json!(transition));
                fields.insert(
                    "uc.outcome".into(),
                    json!(match result {
                        SessionTransitionResult::Completed => "ok",
                        SessionTransitionResult::Skipped => "skipped",
                        SessionTransitionResult::Failed(_) => "error",
                        SessionTransitionResult::Interrupted => "cancelled",
                    }),
                );
                if let SessionTransitionResult::Failed(reason) = result {
                    fields.insert("error.reason".into(), json!(reason));
                }
                if *result == SessionTransitionResult::Interrupted {
                    fields.insert("error.reason".into(), json!("interrupted"));
                }
                fields.insert("duration_ms".into(), json!(duration_ms));
            }
        }
        fields
    }
}
fn insert_failure(fields: &mut Map<String, Value>, failure: Option<(&'static str, &'static str)>) {
    if let Some((phase, reason)) = failure {
        fields.insert("error.phase".into(), json!(phase));
        fields.insert("error.reason".into(), json!(reason));
    }
}
fn exchange_reason(reason: ExchangeFailure) -> &'static str {
    match reason {
        ExchangeFailure::Deferred => "deferred",
        ExchangeFailure::Unavailable => "unavailable",
        ExchangeFailure::AuthenticationRejected => "authentication_rejected",
        ExchangeFailure::ProtocolRejected => "protocol_rejected",
        ExchangeFailure::InvitationUnavailable => "invitation_unavailable",
        ExchangeFailure::PeerUpgradeRequired => "peer_upgrade_required",
    }
}
pub(super) fn dial_reason(reason: DialFailure) -> &'static str {
    match reason {
        DialFailure::TimedOut => "timed_out",
        DialFailure::TransportFailed => "transport_failed",
    }
}

/// 运行时从 SDK 事件解码固定结构；拒绝超长、未知字段、名称不符和伪造级别。
pub fn decode_local_record(name: &str, payload: &str, level: &str) -> Option<Map<String, Value>> {
    if payload.len() > 2048 {
        return None;
    }
    let event: LocalEvent = serde_json::from_str(payload).ok()?;
    (event.name() == name && event.level() == level).then(|| event.fields())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_timeout_is_deferred_and_not_a_peer_rejection() {
        let record = LocalEvent::PresenceCheck {
            result: PresenceCheckResult::Confirmation(ConfirmationFailure::TimedOut),
            duration_ms: 5000,
        };
        assert_eq!(record.fields()["uc.outcome"], "deferred");
        assert_eq!(record.fields()["error.reason"], "timed_out");
    }

    #[test]
    fn typed_record_decoder_rejects_unknown_fields_values_names_and_severity() {
        let payload = r#"{"kind":"presence_closed","direction":"outbound","reason":"remote_application_closed"}"#;
        let fields = decode_local_record("presence.connection.closed", payload, "INFO")
            .expect("valid record");
        assert!(!fields.contains_key("duration_ms"));
        assert!(decode_local_record("presence.connection.closed", payload, "ERROR").is_none());
        assert!(decode_local_record("session.transition.finished", payload, "INFO").is_none());
        assert!(decode_local_record(
            "presence.connection.closed",
            &payload.replace("remote_application_closed", "PRIVATE_REASON"),
            "INFO"
        )
        .is_none());
        assert!(decode_local_record(
            "presence.connection.closed",
            &payload.replace("}", ",\"device\":\"PRIVATE_DEVICE\"}"),
            "INFO"
        )
        .is_none());
        assert!(
            decode_local_record("presence.connection.closed", &"x".repeat(2049), "INFO").is_none()
        );
    }
}
