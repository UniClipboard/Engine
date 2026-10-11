use std::error::Error;
use std::fmt;
use uc_core::error_class::ErrorClass;

/// 固定公开分类；底层失败保留完整来源，不格式化身份或地址。
#[derive(thiserror::Error)]
pub enum ConnectivityError {
    #[error("peer has no usable connection")]
    Offline,
    #[error("connection capacity exhausted")]
    Busy(#[source] tokio::sync::TryAcquireError),
    #[error("connection owner stopped")]
    Stopped,
    #[error("connection resource owner closed")]
    ResourceClosed(#[source] tokio::sync::AcquireError),
    #[error("connection result channel closed")]
    ResultClosed(#[source] tokio::sync::watch::error::RecvError),
    #[error("connection task failed")]
    Task(#[source] std::sync::Arc<tokio::task::JoinError>),
    #[error("single-exchange connection already used")]
    ExchangeAlreadyUsed,
    #[error("peer admission denied")]
    Denied,
    #[error("connection operation deadline expired")]
    DeadlineExpired,
    #[error("connection operation timed out")]
    TimedOut(#[source] tokio::time::error::Elapsed),
    #[error("connection address lookup failed")]
    AddressLookup(#[source] Box<dyn Error + Send + Sync>),
    #[error("connection backend failed")]
    Backend(#[source] Box<dyn Error + Send + Sync>),
    #[error("connection admission failed")]
    Admission(#[source] Box<dyn Error + Send + Sync>),
    #[error("connection stream failed")]
    Stream(#[source] Box<dyn Error + Send + Sync>),
    #[error("shared connection acquisition failed")]
    Shared(#[source] std::sync::Arc<ConnectivityError>),
}

impl fmt::Debug for ConnectivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl ConnectivityError {
    /// 稳定业务端口仍把无法取得可用连接归类为离线；资源与任务失败保留来源。
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Offline | Self::Backend(_) | Self::Denied => true,
            Self::Shared(error) => error.is_unavailable(),
            _ => false,
        }
    }
}

impl ErrorClass for ConnectivityError {
    fn class(&self) -> &'static str {
        match self {
            Self::Offline => "connection_offline",
            Self::Busy(_) => "connection_busy",
            Self::Stopped => "connection_stopped",
            Self::ResourceClosed(_) => "connection_resource_closed",
            Self::ResultClosed(_) => "connection_result_closed",
            Self::Task(_) => "connection_task_failed",
            Self::ExchangeAlreadyUsed => "connection_exchange_already_used",
            Self::Denied => "connection_admission_denied",
            Self::DeadlineExpired | Self::TimedOut(_) => "connection_timeout",
            Self::AddressLookup(_) => "connection_address_lookup_failed",
            Self::Backend(_) => "connection_backend_failed",
            Self::Admission(_) => "connection_admission_failed",
            Self::Stream(_) => "connection_stream_failed",
            Self::Shared(source) => source.class(),
        }
    }
}
