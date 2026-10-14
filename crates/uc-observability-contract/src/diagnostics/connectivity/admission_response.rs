//! Joiner 等待 `OpaqueResponse` 失败的本地详情：保留读取失败的具体类别，不改变公开结果。
use super::super::{CompletionResult, DiagnosticErrorType, OperationCompletion};
use super::authentication::{LocalCompletionDetail, PendingCompletionDetail};

/// Sponsor 在应用层关闭连接时的关闭码类别；只有固定类别，不记录原始数值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationCloseClass {
    SessionUnavailable,
    AuthenticationRejected,
    Busy,
    PeerUpgradeRequired,
    Protocol,
    Other,
}

/// 发出初次 hello 之后、读取 `OpaqueResponse` 失败的具体类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionResponseFailure {
    /// 在读响应的总截止内 sponsor 没有应答。
    ResponseTimeout,
    /// 对端重置了响应流。
    StreamReset,
    /// 连接在应答前丢失（超时、被对端重置、被本地关闭等）。
    ConnectionLost,
    /// 对端以应用层关闭码关闭了连接。
    ApplicationClosed(ApplicationCloseClass),
    /// 收到了无法解码的响应帧。
    Decode,
}

impl ApplicationCloseClass {
    fn reason(self) -> &'static str {
        match self {
            Self::SessionUnavailable => "app_close_session_unavailable",
            Self::AuthenticationRejected => "app_close_authentication",
            Self::Busy => "app_close_busy",
            Self::PeerUpgradeRequired => "app_close_peer_upgrade_required",
            Self::Protocol => "app_close_protocol",
            Self::Other => "app_close_other",
        }
    }
}

impl AdmissionResponseFailure {
    pub(super) fn local_fields(self) -> (&'static str, &'static str) {
        let reason = match self {
            Self::ResponseTimeout => "response_timeout",
            Self::StreamReset => "stream_reset",
            Self::ConnectionLost => "connection_lost",
            Self::ApplicationClosed(class) => class.reason(),
            Self::Decode => "decode",
        };
        ("receive_response", reason)
    }

    /// 需要改写完成记录错误类别的失败；应用层关闭保持调用方按公开结果得出的类别。
    fn error_type(self) -> Option<DiagnosticErrorType> {
        match self {
            Self::ResponseTimeout => Some(DiagnosticErrorType::Timeout),
            Self::StreamReset | Self::ConnectionLost => Some(DiagnosticErrorType::ChannelClosed),
            Self::Decode => Some(DiagnosticErrorType::DecodeFailed),
            Self::ApplicationClosed(_) => None,
        }
    }
}

/// 在初次交换的完成记录上附加响应读取失败的本地详情；详情只进本地文件，不进远程属性。
///
/// 完成记录的错误类别随具体失败修正（例如响应超时是 `timeout`，不是 `authentication_failed`）；
/// 调用方返回给 Application 的公开结果不变。
pub fn complete_admission_response_failure(
    failure: AdmissionResponseFailure,
    mut completion: OperationCompletion,
) {
    if let (Some(error_type), CompletionResult::Failed(_)) =
        (failure.error_type(), &completion.result)
    {
        completion.result = CompletionResult::Failed(error_type);
    }
    let context = opentelemetry::Context::current().with_value(PendingCompletionDetail {
        detail: LocalCompletionDetail::AdmissionResponse(failure),
        consumed: std::sync::atomic::AtomicBool::new(false),
    });
    let _guard = context.attach();
    super::super::complete_operation(completion);
}
