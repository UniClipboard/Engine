//! 认证完成的本地详情：复用一次标准完成事件，来源详情不进入远程属性。
use super::super::{complete_operation, ObservationContext, OperationCompletion};
use super::record::{dial_reason, DialFailure};
use super::ClipboardReceiveFailure;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialFailure {
    RecordMissing,
    CredentialMissing,
    Locked,
    Corrupt,
    Unavailable,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationFailure {
    ReadHello(ReadFailure),
    InitialCredential(CredentialFailure),
    ContinuationCredential(CredentialFailure),
    IdentityMismatch(IdentityCheck),
    InitialProof(ProofFailure),
    ContinuationProof(ProofFailure),
    ReadRequest(ReadFailure),
    RequestProof(ProofFailure),
    DeadlineExceeded(AuthenticationStage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailure {
    TimedOut,
    TransportFailed,
    InvalidMessage,
    PeerUpgradeRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofFailure {
    Rejected,
    Exchange(ReadFailure),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCheck {
    InitialVersion,
    InitialPeer,
    ContinuationPeer,
    RequestBinding,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationStage {
    ReceiveHello,
    InitialCredential,
    InitialProof,
    ContinuationCredential,
    ReceiveRequest,
}

pub fn complete_admission_authentication_failure(
    failure: AuthenticationFailure,
    duration: Duration,
) {
    let context = opentelemetry::Context::new().with_value(PendingCompletionDetail {
        detail: LocalCompletionDetail::Authentication(failure),
        consumed: std::sync::atomic::AtomicBool::new(false),
    });
    let _guard = context.attach();
    super::super::emit_unassociated_operation(super::super::OperationCompletion::failed(
        super::super::DiagnosticDomain::SpaceAdmission,
        super::super::DiagnosticOperation::NetworkTransport,
        super::super::DiagnosticRole::Sponsor,
        failure.error_type(),
        duration,
    ));
}

struct PendingCompletionDetail {
    detail: LocalCompletionDetail,
    consumed: std::sync::atomic::AtomicBool,
}

impl CredentialFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::RecordMissing => "record_missing",
            Self::CredentialMissing => "credential_missing",
            Self::Locked => "storage_locked",
            Self::Corrupt => "storage_corrupt",
            Self::Unavailable => "storage_unavailable",
            Self::RecoveryRequired => "storage_recovery_required",
        }
    }
}
impl ReadFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::TimedOut => "timed_out",
            Self::TransportFailed => "transport_failed",
            Self::InvalidMessage => "invalid_message",
            Self::PeerUpgradeRequired => "peer_upgrade_required",
        }
    }
    fn error_type(self) -> super::super::DiagnosticErrorType {
        match self {
            Self::TimedOut => super::super::DiagnosticErrorType::Timeout,
            Self::PeerUpgradeRequired => super::super::DiagnosticErrorType::PeerIncompatible,
            _ => super::super::DiagnosticErrorType::DecodeFailed,
        }
    }
}
impl ProofFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "proof_rejected",
            Self::Exchange(reason) => reason.as_str(),
        }
    }
}
impl AuthenticationStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReceiveHello => "receive_hello",
            Self::InitialCredential => "initial_credential",
            Self::InitialProof => "initial_proof",
            Self::ContinuationCredential => "continuation_credential",
            Self::ReceiveRequest => "receive_request",
        }
    }
}
impl AuthenticationFailure {
    fn local_fields(self) -> (&'static str, &'static str) {
        match self {
            Self::InitialCredential(reason) => ("initial_credential", reason.as_str()),
            Self::ContinuationCredential(reason) => ("continuation_credential", reason.as_str()),
            Self::ReadHello(reason) => ("receive_hello", reason.as_str()),
            Self::ReadRequest(reason) => ("receive_request", reason.as_str()),
            Self::InitialProof(reason) => ("initial_proof", reason.as_str()),
            Self::ContinuationProof(reason) => ("continuation_proof", reason.as_str()),
            Self::RequestProof(reason) => ("request_proof", reason.as_str()),
            Self::DeadlineExceeded(stage) => (stage.as_str(), "timed_out"),
            Self::IdentityMismatch(check) => (
                match check {
                    IdentityCheck::InitialVersion => "initial_version",
                    IdentityCheck::InitialPeer => "initial_identity",
                    IdentityCheck::ContinuationPeer => "continuation_identity",
                    IdentityCheck::RequestBinding => "request_identity",
                },
                "identity_mismatch",
            ),
        }
    }
    fn error_type(self) -> super::super::DiagnosticErrorType {
        match self {
            Self::ReadHello(reason)
            | Self::ReadRequest(reason)
            | Self::InitialProof(ProofFailure::Exchange(reason))
            | Self::ContinuationProof(ProofFailure::Exchange(reason))
            | Self::RequestProof(ProofFailure::Exchange(reason)) => reason.error_type(),
            Self::DeadlineExceeded(_) => super::super::DiagnosticErrorType::Timeout,
            _ => super::super::DiagnosticErrorType::AuthenticationFailed,
        }
    }
}

/// 仅本地 SDK 处理器在同步投递时调用；详情不进入日志属性、队列上下文或网络。
/// 匹配原有完整认证结果后消费一次，嵌套 Context 退出时自动恢复外层附件。
pub fn take_local_completion_detail(
    domain: &str,
    operation: &str,
    role: &str,
    outcome: &str,
) -> Option<LocalCompletionDetail> {
    opentelemetry::Context::map_current(|context| {
        let pending = context.get::<PendingCompletionDetail>()?;
        let expected = match pending.detail {
            LocalCompletionDetail::ClipboardReceive(_) => {
                ("clipboard", "clipboard_receive", "server", "error")
            }
            LocalCompletionDetail::GroupUpdate(_) => (
                "space_membership",
                "membership_group_update",
                "member",
                "error",
            ),
            LocalCompletionDetail::Authentication(_) => {
                ("space_admission", "network_transport", "sponsor", "error")
            }
            LocalCompletionDetail::AdmissionConnection(_) => {
                ("space_admission", "space_admission", "joiner", "deferred")
            }
        };
        if (domain, operation, role, outcome) != expected {
            return None;
        }
        (!pending
            .consumed
            .swap(true, std::sync::atomic::Ordering::AcqRel))
        .then_some(pending.detail)
    })
}

#[derive(Clone, Copy)]
pub enum LocalCompletionDetail {
    ClipboardReceive(ClipboardReceiveFailure),
    Authentication(AuthenticationFailure),
    AdmissionConnection(DialFailure),
    GroupUpdate(super::GroupUpdateFailureDetail),
}
impl LocalCompletionDetail {
    pub fn local_fields(self) -> (&'static str, &'static str) {
        match self {
            Self::ClipboardReceive(failure) => failure.local_fields(),
            Self::Authentication(failure) => failure.local_fields(),
            Self::AdmissionConnection(reason) => ("connect", dial_reason(reason)),
            Self::GroupUpdate(detail) => (detail.phase.as_str(), detail.reason.as_str()),
        }
    }

    pub fn source_chain(self) -> Option<[&'static str; 4]> {
        match self {
            Self::ClipboardReceive(failure) => failure.source_chain(),
            Self::GroupUpdate(detail) => Some([
                "membership_update",
                detail.phase.as_str(),
                detail.source.as_str(),
                detail.reason.as_str(),
            ]),
            _ => None,
        }
    }
}

pub fn complete_clipboard_receive_failure(
    failure: ClipboardReceiveFailure,
    completion: OperationCompletion,
) {
    let context = opentelemetry::Context::current().with_value(PendingCompletionDetail {
        detail: LocalCompletionDetail::ClipboardReceive(failure),
        consumed: AtomicBool::new(false),
    });
    let _guard = context.attach();
    complete_operation(completion);
}

pub fn complete_group_update_failure(
    detail: super::GroupUpdateFailureDetail,
    completion: super::super::OperationCompletion,
) {
    let context = opentelemetry::Context::current().with_value(PendingCompletionDetail {
        detail: LocalCompletionDetail::GroupUpdate(detail),
        consumed: std::sync::atomic::AtomicBool::new(false),
    });
    let _guard = context.attach();
    super::super::complete_operation(completion);
}
pub fn complete_admission_connection_failure(reason: DialFailure, duration: Duration) {
    let context = ObservationContext::capture()
        .0
        .with_value(PendingCompletionDetail {
            detail: LocalCompletionDetail::AdmissionConnection(reason),
            consumed: std::sync::atomic::AtomicBool::new(false),
        });
    let _guard = context.attach();
    super::super::complete_operation(super::super::OperationCompletion::deferred(
        super::super::DiagnosticDomain::SpaceAdmission,
        super::super::DiagnosticOperation::SpaceAdmission,
        super::super::DiagnosticRole::Joiner,
        duration,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_consumption_is_matched_once_and_nested_contexts_restore_the_outer_detail() {
        let outer_failure = AuthenticationFailure::InitialCredential(CredentialFailure::Locked);
        let outer = opentelemetry::Context::new().with_value(PendingCompletionDetail {
            detail: LocalCompletionDetail::Authentication(outer_failure),
            consumed: std::sync::atomic::AtomicBool::new(false),
        });
        let outer_guard = outer.attach();
        assert!(
            take_local_completion_detail("clipboard", "clipboard_dispatch", "client", "error")
                .is_none()
        );
        {
            let inner = opentelemetry::Context::new().with_value(PendingCompletionDetail {
                detail: LocalCompletionDetail::Authentication(
                    AuthenticationFailure::ContinuationCredential(CredentialFailure::RecordMissing),
                ),
                consumed: std::sync::atomic::AtomicBool::new(false),
            });
            let _inner_guard = inner.attach();
            assert_eq!(
                take_local_completion_detail(
                    "space_admission",
                    "network_transport",
                    "sponsor",
                    "error"
                )
                .expect("inner")
                .local_fields(),
                ("continuation_credential", "record_missing")
            );
            assert!(take_local_completion_detail(
                "space_admission",
                "network_transport",
                "sponsor",
                "error"
            )
            .is_none());
        }
        assert_eq!(
            take_local_completion_detail(
                "space_admission",
                "network_transport",
                "sponsor",
                "error"
            )
            .expect("outer")
            .local_fields(),
            outer_failure.local_fields()
        );
        drop(outer_guard);
        assert!(take_local_completion_detail(
            "space_admission",
            "network_transport",
            "sponsor",
            "error"
        )
        .is_none());
    }
}
