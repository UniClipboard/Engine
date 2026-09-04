//! 运行诊断的稳定、低基数字段合同。
//!
//! 本模块只负责约束记录形状，不安装 subscriber，也不选择或连接后端。

use std::fmt;
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;
pub const TELEMETRY_TARGET: &str = "uc.telemetry";

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DiagnosticFlowId([u8; 16]);

impl DiagnosticFlowId {
    /// 从业务 owner 已有的随机尝试材料派生不可逆的诊断关联号。
    pub fn derive(purpose: DiagnosticFlowPurpose, source: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/diagnostic-flow/v1\0");
        hasher.update(purpose.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(source);
        let digest = hasher.finalize();
        let mut value = [0_u8; 16];
        value.copy_from_slice(&digest[..16]);
        Self(value)
    }
}

impl fmt::Debug for DiagnosticFlowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DiagnosticFlowId(REDACTED)")
    }
}

struct FlowAttribute<'a>(&'a DiagnosticFlowId);

impl fmt::Display for FlowAttribute<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 .0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticFlowPurpose {
    ClipboardSync,
    SpaceAdmission,
    SpaceMembership,
}

impl DiagnosticFlowPurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClipboardSync => "clipboard_sync",
            Self::SpaceAdmission => "space_admission",
            Self::SpaceMembership => "space_membership",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticDomain {
    Clipboard,
    SpaceAdmission,
    SpaceMembership,
    Storage,
    Runtime,
}

impl DiagnosticDomain {
    fn as_str(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::SpaceAdmission => "space_admission",
            Self::SpaceMembership => "space_membership",
            Self::Storage => "storage",
            Self::Runtime => "runtime",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticOperation {
    ClipboardDispatch,
    ClipboardReceive,
    ClipboardAddressResolve,
    ClipboardConnect,
    SpaceAdmission,
    MembershipHistorySync,
    MembershipGroupUpdate,
    ProfileStorageUpgrade,
    SessionLifecycle,
}

impl DiagnosticOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClipboardDispatch => "clipboard_dispatch",
            Self::ClipboardReceive => "clipboard_receive",
            Self::ClipboardAddressResolve => "clipboard_address_resolve",
            Self::ClipboardConnect => "clipboard_connect",
            Self::SpaceAdmission => "space_admission",
            Self::MembershipHistorySync => "membership_history_sync",
            Self::MembershipGroupUpdate => "membership_group_update",
            Self::ProfileStorageUpgrade => "profile_storage_upgrade",
            Self::SessionLifecycle => "session_lifecycle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticRole {
    Local,
    Client,
    Server,
    Joiner,
    Sponsor,
    Member,
}

impl DiagnosticRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Client => "client",
            Self::Server => "server",
            Self::Joiner => "joiner",
            Self::Sponsor => "sponsor",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSpanKind {
    Internal,
    Client,
    Server,
}

impl DiagnosticSpanKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticErrorType {
    AuthenticationFailed,
    AddressUnavailable,
    ConnectFailed,
    StreamFailed,
    DecodeFailed,
    Timeout,
    ChannelClosed,
    Storage,
    Security,
    Corrupt,
    SourceChanged,
    Manifest,
    JoinFailed,
    ShutdownTimeout,
    Unavailable,
    PeerRejected,
    PeerIncompatible,
    LocalPolicyExceeded,
    Internal,
}

impl DiagnosticErrorType {
    fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "authentication_failed",
            Self::AddressUnavailable => "address_unavailable",
            Self::ConnectFailed => "connect_failed",
            Self::StreamFailed => "stream_failed",
            Self::DecodeFailed => "decode_failed",
            Self::Timeout => "timeout",
            Self::ChannelClosed => "channel_closed",
            Self::Storage => "storage",
            Self::Security => "security",
            Self::Corrupt => "corrupt",
            Self::SourceChanged => "source_changed",
            Self::Manifest => "manifest",
            Self::JoinFailed => "join_failed",
            Self::ShutdownTimeout => "shutdown_timeout",
            Self::Unavailable => "unavailable",
            Self::PeerRejected => "peer_rejected",
            Self::PeerIncompatible => "peer_incompatible",
            Self::LocalPolicyExceeded => "local_policy_exceeded",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OperationContext<'a> {
    pub domain: DiagnosticDomain,
    pub operation: DiagnosticOperation,
    pub role: DiagnosticRole,
    pub kind: DiagnosticSpanKind,
    pub flow: Option<&'a DiagnosticFlowId>,
}

pub fn operation_span(context: OperationContext<'_>) -> tracing::Span {
    match context.flow {
        Some(flow) => tracing::span!(
            target: TELEMETRY_TARGET,
            tracing::Level::INFO,
            "uc.operation",
            uc.domain = context.domain.as_str(),
            uc.operation = context.operation.as_str(),
            uc.role = context.role.as_str(),
            uc.flow.id = %FlowAttribute(flow),
            otel.kind = context.kind.as_str(),
        ),
        None => tracing::span!(
            target: TELEMETRY_TARGET,
            tracing::Level::INFO,
            "uc.operation",
            uc.domain = context.domain.as_str(),
            uc.operation = context.operation.as_str(),
            uc.role = context.role.as_str(),
            otel.kind = context.kind.as_str(),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionResult {
    Succeeded,
    Failed(DiagnosticErrorType),
    Deferred,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
pub struct OperationCompletion {
    domain: DiagnosticDomain,
    operation: DiagnosticOperation,
    role: DiagnosticRole,
    result: CompletionResult,
    duration: Duration,
}

impl OperationCompletion {
    pub fn succeeded(
        domain: DiagnosticDomain,
        operation: DiagnosticOperation,
        role: DiagnosticRole,
        duration: Duration,
    ) -> Self {
        Self {
            domain,
            operation,
            role,
            result: CompletionResult::Succeeded,
            duration,
        }
    }

    pub fn failed(
        domain: DiagnosticDomain,
        operation: DiagnosticOperation,
        role: DiagnosticRole,
        error_type: DiagnosticErrorType,
        duration: Duration,
    ) -> Self {
        Self {
            domain,
            operation,
            role,
            result: CompletionResult::Failed(error_type),
            duration,
        }
    }

    pub fn deferred(
        domain: DiagnosticDomain,
        operation: DiagnosticOperation,
        role: DiagnosticRole,
        duration: Duration,
    ) -> Self {
        Self {
            domain,
            operation,
            role,
            result: CompletionResult::Deferred,
            duration,
        }
    }

    pub fn rejected(
        domain: DiagnosticDomain,
        operation: DiagnosticOperation,
        role: DiagnosticRole,
        duration: Duration,
    ) -> Self {
        Self {
            domain,
            operation,
            role,
            result: CompletionResult::Rejected,
            duration,
        }
    }

    pub fn cancelled(
        domain: DiagnosticDomain,
        operation: DiagnosticOperation,
        role: DiagnosticRole,
        duration: Duration,
    ) -> Self {
        Self {
            domain,
            operation,
            role,
            result: CompletionResult::Cancelled,
            duration,
        }
    }
}

pub fn complete_operation(completion: OperationCompletion) {
    let duration_ms = u64::try_from(completion.duration.as_millis()).unwrap_or(u64::MAX);
    let domain = completion.domain.as_str();
    let operation = completion.operation.as_str();
    let role = completion.role.as_str();
    match completion.result {
        CompletionResult::Succeeded => tracing::event!(
            target: TELEMETRY_TARGET,
            tracing::Level::INFO,
            event.name = "uc.operation.completed",
            uc.domain = domain,
            uc.operation = operation,
            uc.role = role,
            uc.outcome = "ok",
            duration_ms,
        ),
        CompletionResult::Failed(error_type) => tracing::event!(
            target: TELEMETRY_TARGET,
            tracing::Level::ERROR,
            event.name = "uc.operation.completed",
            uc.domain = domain,
            uc.operation = operation,
            uc.role = role,
            uc.outcome = "error",
            error.type = error_type.as_str(),
            duration_ms,
        ),
        CompletionResult::Deferred => {
            record_non_error_completion(domain, operation, role, "deferred", duration_ms)
        }
        CompletionResult::Rejected => {
            record_non_error_completion(domain, operation, role, "rejected", duration_ms)
        }
        CompletionResult::Cancelled => {
            record_non_error_completion(domain, operation, role, "cancelled", duration_ms)
        }
    }
}

fn record_non_error_completion(
    domain: &'static str,
    operation: &'static str,
    role: &'static str,
    outcome: &'static str,
    duration_ms: u64,
) {
    tracing::event!(
        target: TELEMETRY_TARGET,
        tracing::Level::INFO,
        event.name = "uc.operation.completed",
        uc.domain = domain,
        uc.operation = operation,
        uc.role = role,
        uc.outcome = outcome,
        duration_ms,
    );
}
