//! 运行诊断的稳定、低基数字段合同。
//!
//! 本模块只负责约束记录形状，不安装 subscriber，也不选择或连接后端。

use std::fmt;
use std::future::Future;
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;
pub const TELEMETRY_TARGET: &str = "uc.telemetry";
pub const HEALTH_TARGET: &str = "observability.health";

pub fn managed_log_file_name(date: chrono::NaiveDate) -> String {
    format!("engine.{}.jsonl", date.format("%Y-%m-%d"))
}

pub fn managed_log_file_date(name: &str) -> Option<chrono::NaiveDate> {
    let date = name.strip_prefix("engine.")?.strip_suffix(".jsonl")?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

#[derive(Clone)]
struct DiagnosticFlowId([u8; 16]);

impl DiagnosticFlowId {
    /// 从业务 owner 已有的随机尝试材料派生不可逆的诊断关联号。
    fn derive_space_admission(source: &[u8; 32]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/diagnostic-flow/v1\0");
        hasher.update(b"space_admission");
        hasher.update(b"\0");
        hasher.update(source);
        let digest = hasher.finalize();
        let mut value = [0_u8; 16];
        value.copy_from_slice(&digest[..16]);
        Self(value)
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

tokio::task_local! {
    static SPACE_ADMISSION_FLOW: DiagnosticFlowId;
}

/// 在完整 Space 准入 owner 内提供不可读取的跨重试关联作用域。
pub async fn scope_space_admission_observation<T>(
    attempt_material: &[u8; 32],
    future: impl Future<Output = T>,
) -> T {
    SPACE_ADMISSION_FLOW
        .scope(
            DiagnosticFlowId::derive_space_admission(attempt_material),
            future,
        )
        .await
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
    NetworkTransport,
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
            Self::NetworkTransport => "network_transport",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticTaskKind {
    ClipboardInboundOsWrite,
    ActiveClipboardConverge,
    ClipboardDeferredDrain,
    PairingMdnsForward,
    MobileOutboundDispatch,
}

impl DiagnosticTaskKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClipboardInboundOsWrite => "clipboard_inbound_os_write",
            Self::ActiveClipboardConverge => "active_clipboard_converge",
            Self::ClipboardDeferredDrain => "clipboard_deferred_drain",
            Self::PairingMdnsForward => "pairing_mdns_forward",
            Self::MobileOutboundDispatch => "mobile_outbound_dispatch",
        }
    }
}

pub fn record_task_join_failure(task: DiagnosticTaskKind) {
    tracing::event!(
        target: "observability.health",
        parent: None,
        tracing::Level::WARN,
        event.name = "uc.task.join_failed",
        task.kind = task.as_str(),
        error.type = "join_failed",
    );
}

pub fn record_task_shutdown(
    completed_count: usize,
    timed_out_count: usize,
    join_error_count: usize,
) {
    tracing::event!(
        target: "observability.health",
        parent: None,
        tracing::Level::INFO,
        event.name = "uc.task.shutdown",
        task.completed.count = u64::try_from(completed_count).unwrap_or(u64::MAX),
        task.timed_out.count = u64::try_from(timed_out_count).unwrap_or(u64::MAX),
        task.join_error.count = u64::try_from(join_error_count).unwrap_or(u64::MAX),
    );
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
pub struct OperationContext {
    pub domain: DiagnosticDomain,
    pub operation: DiagnosticOperation,
    pub role: DiagnosticRole,
    pub kind: DiagnosticSpanKind,
}

pub fn operation_span(context: OperationContext) -> tracing::Span {
    let flow = matches!(
        (
            context.domain,
            context.operation,
            context.role,
            context.kind,
        ),
        (
            DiagnosticDomain::SpaceAdmission,
            DiagnosticOperation::SpaceAdmission | DiagnosticOperation::NetworkTransport,
            DiagnosticRole::Joiner,
            DiagnosticSpanKind::Client,
        )
    )
    .then(|| SPACE_ADMISSION_FLOW.try_with(Clone::clone).ok())
    .flatten();
    match flow.as_ref() {
        Some(flow) => tracing::span!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            "uc.operation",
            otel.name = context.operation.as_str(),
            uc.domain = context.domain.as_str(),
            uc.operation = context.operation.as_str(),
            uc.role = context.role.as_str(),
            uc.flow.id = %FlowAttribute(flow),
            otel.kind = context.kind.as_str(),
            otel.status_code = tracing::field::Empty,
        ),
        None => tracing::span!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            "uc.operation",
            otel.name = context.operation.as_str(),
            uc.domain = context.domain.as_str(),
            uc.operation = context.operation.as_str(),
            uc.role = context.role.as_str(),
            otel.kind = context.kind.as_str(),
            otel.status_code = tracing::field::Empty,
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
        CompletionResult::Succeeded => {
            tracing::Span::current().record("otel.status_code", "OK");
        }
        CompletionResult::Failed(_) => {
            tracing::Span::current().record("otel.status_code", "ERROR");
        }
        CompletionResult::Deferred | CompletionResult::Rejected | CompletionResult::Cancelled => {}
    }
    match completion.result {
        CompletionResult::Succeeded => tracing::event!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            event.name = "uc.operation.completed",
            uc.domain = domain,
            uc.operation = operation,
            uc.role = role,
            uc.outcome = "ok",
            duration_ms,
        ),
        CompletionResult::Failed(error_type) => tracing::event!(
            target: "uc.telemetry",
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

pub fn complete_unassociated_operation(completion: OperationCompletion) {
    let duration_ms = u64::try_from(completion.duration.as_millis()).unwrap_or(u64::MAX);
    let domain = completion.domain.as_str();
    let operation = completion.operation.as_str();
    let role = completion.role.as_str();
    match completion.result {
        CompletionResult::Succeeded => tracing::event!(
            target: "uc.telemetry",
            parent: None,
            tracing::Level::INFO,
            event.name = "uc.operation.completed",
            uc.domain = domain,
            uc.operation = operation,
            uc.role = role,
            uc.outcome = "ok",
            duration_ms,
        ),
        CompletionResult::Failed(error_type) => tracing::event!(
            target: "uc.telemetry",
            parent: None,
            tracing::Level::ERROR,
            event.name = "uc.operation.completed",
            uc.domain = domain,
            uc.operation = operation,
            uc.role = role,
            uc.outcome = "error",
            error.type = error_type.as_str(),
            duration_ms,
        ),
        CompletionResult::Deferred => record_unassociated_non_error_completion(
            domain,
            operation,
            role,
            "deferred",
            duration_ms,
        ),
        CompletionResult::Rejected => record_unassociated_non_error_completion(
            domain,
            operation,
            role,
            "rejected",
            duration_ms,
        ),
        CompletionResult::Cancelled => record_unassociated_non_error_completion(
            domain,
            operation,
            role,
            "cancelled",
            duration_ms,
        ),
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
        target: "uc.telemetry",
        tracing::Level::INFO,
        event.name = "uc.operation.completed",
        uc.domain = domain,
        uc.operation = operation,
        uc.role = role,
        uc.outcome = outcome,
        duration_ms,
    );
}

fn record_unassociated_non_error_completion(
    domain: &'static str,
    operation: &'static str,
    role: &'static str,
    outcome: &'static str,
    duration_ms: u64,
) {
    tracing::event!(
        target: "uc.telemetry",
        parent: None,
        tracing::Level::INFO,
        event.name = "uc.operation.completed",
        uc.domain = domain,
        uc.operation = operation,
        uc.role = role,
        uc.outcome = outcome,
        duration_ms,
    );
}
