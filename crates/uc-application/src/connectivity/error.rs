use super::{ElapsedClockError, InboundConnectionError};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use uc_core::error_class::ErrorClass;

/// 固定公开分类；底层失败保留完整来源，不格式化身份或地址。
#[derive(thiserror::Error)]
pub enum ConnectivityError {
    #[error("connection retention clock unavailable")]
    Clock(#[source] Arc<ElapsedClockError>),
    #[error("peer has no known address")]
    AddressMissing,
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
    #[error("inbound connection result channel closed")]
    InboundResultClosed(#[source] tokio::sync::oneshot::error::RecvError),
    #[error("connection task failed")]
    Task(#[source] std::sync::Arc<tokio::task::JoinError>),
    #[error("single-exchange connection already used")]
    ExchangeAlreadyUsed,
    #[error("connection stream capability unavailable")]
    Capability(#[source] StreamCapabilityRefusal),
    #[error("peer admission denied")]
    Denied,
    #[error("connection operation deadline expired")]
    DeadlineExpired,
    #[error("connection operation timed out")]
    TimedOut(#[source] tokio::time::error::Elapsed),
    #[error("connection admission read deadline expired")]
    AdmissionDeadlineExpired,
    #[error("connection admission read timed out")]
    AdmissionTimedOut(#[source] tokio::time::error::Elapsed),
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
    #[error("connection local work settlement failed")]
    Settlement {
        #[source]
        primary: Box<ConnectivityError>,
        additional: Box<ConnectivityError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum StreamCapabilityRefusal {
    #[error("unidirectional streams unavailable")]
    UnidirectionalUnavailable,
}

impl fmt::Debug for ConnectivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl ConnectivityError {
    /// 稳定业务端口仍把无法取得可用连接归类为离线；资源与任务失败保留来源。
    pub fn is_admission_read_failure(&self) -> bool {
        match self {
            Self::Admission(_) | Self::AdmissionTimedOut(_) | Self::AdmissionDeadlineExpired => {
                true
            }
            Self::Shared(source) => source.is_admission_read_failure(),
            Self::Settlement {
                primary,
                additional,
            } => primary.is_admission_read_failure() || additional.is_admission_read_failure(),
            _ => false,
        }
    }
    pub fn is_address_missing(&self) -> bool {
        match self {
            Self::AddressMissing => true,
            Self::Shared(source) => source.is_address_missing(),
            _ => false,
        }
    }
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Offline | Self::AddressMissing | Self::Backend(_) | Self::Denied => true,
            Self::Shared(error) => error.is_unavailable(),
            _ => false,
        }
    }
}

impl ErrorClass for ConnectivityError {
    fn class(&self) -> &'static str {
        match self {
            Self::Clock(source) => source.class(),
            Self::AddressMissing => "connection_address_missing",
            Self::Offline => "connection_offline",
            Self::Busy(_) => "connection_busy",
            Self::Stopped => "connection_stopped",
            Self::ResourceClosed(_) => "connection_resource_closed",
            Self::ResultClosed(_) => "connection_result_closed",
            Self::InboundResultClosed(_) => "inbound_connection_result_closed",
            Self::Task(_) => "connection_task_failed",
            Self::ExchangeAlreadyUsed => "connection_exchange_already_used",
            Self::Capability(_) => "connection_stream_capability_unavailable",
            Self::Denied => "connection_admission_denied",
            Self::DeadlineExpired | Self::TimedOut(_) => "connection_timeout",
            Self::AdmissionTimedOut(_) | Self::AdmissionDeadlineExpired => {
                "connection_admission_timeout"
            }
            Self::AddressLookup(_) => "connection_address_lookup_failed",
            Self::Backend(_) => "connection_backend_failed",
            Self::Admission(_) => "connection_admission_failed",
            Self::Stream(_) => "connection_stream_failed",
            Self::Shared(source) => source.class(),
            Self::Settlement { .. } => "connection_settlement_failed",
        }
    }
}

impl From<InboundConnectionError> for ConnectivityError {
    fn from(source: InboundConnectionError) -> Self {
        // 识别入口的临时许可在错误交接时释放，保留原始分类和完整来源。
        source.cause
    }
}

#[cfg(test)]
mod inbound_handoff_tests {
    use super::{ConnectivityError, InboundConnectionError};
    use std::error::Error;
    use std::io;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    #[test]
    fn consuming_inbound_failure_releases_reservation_and_keeps_original_source() {
        let capacity = Arc::new(Semaphore::new(1));
        let error = InboundConnectionError {
            cause: ConnectivityError::Admission(Box::new(io::Error::from(
                io::ErrorKind::PermissionDenied,
            ))),
            _capacity: Some(capacity.clone().try_acquire_owned().unwrap()),
        };
        assert_eq!(capacity.available_permits(), 0);
        let error = ConnectivityError::from(error);
        assert_eq!(capacity.available_permits(), 1);
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
