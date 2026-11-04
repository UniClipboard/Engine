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
    /// 请求的撤销快照已失效（例如锁定准备暂停全部连接），但成员权威未确认拒绝。旧请求仍被
    /// 围栏拒绝；只有新请求才会重新读取当前成员权威。
    ///
    /// 撤销快照失效后读取成员权威失败时保留该来源；确认仍准入时没有来源。
    #[error("peer admission request invalidated")]
    Invalidated(#[source] Option<Box<dyn Error + Send + Sync>>),
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
            Self::Offline
            | Self::AddressMissing
            | Self::Backend(_)
            | Self::Denied
            | Self::Invalidated(_) => true,
            Self::Shared(error) => error.is_unavailable(),
            _ => false,
        }
    }
    /// 准入授权拒绝（含经共享拨号结果返回的拒绝）：成员权威拒绝或身份不符。`is_unavailable`
    /// 为既有调用方把它算作不可用；需要区分终态授权失败的边界必须先用本判断。请求失效
    /// （`Invalidated`）不是拒绝。
    pub fn is_denied(&self) -> bool {
        match self {
            Self::Denied => true,
            Self::Shared(error) => error.is_denied(),
            _ => false,
        }
    }
    /// 暂时性的传输或容量失败：稍后重试同一请求可能成功。准入拒绝、本机负责人
    /// 停止、本机时钟失效、流能力不可用、地址资料读取失败与本机结算失败都不属于此类。
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Busy(_)
            | Self::DeadlineExpired
            | Self::TimedOut(_)
            | Self::AdmissionDeadlineExpired
            | Self::AdmissionTimedOut(_)
            | Self::Stream(_) => true,
            Self::Shared(error) => error.is_transient(),
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
            Self::Invalidated(_) => "connection_admission_invalidated",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 只有暂时性的传输与容量失败可重试；准入拒绝、本机停止与地址资料失败不可重试。
    #[tokio::test]
    async fn only_transport_and_capacity_failures_are_transient() {
        let busy = std::sync::Arc::new(tokio::sync::Semaphore::new(0))
            .try_acquire_owned()
            .map(drop)
            .unwrap_err();
        let elapsed = tokio::time::timeout(std::time::Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err();
        let stream = || Box::new(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        for error in [
            ConnectivityError::Busy(busy),
            ConnectivityError::DeadlineExpired,
            ConnectivityError::TimedOut(elapsed),
            ConnectivityError::AdmissionDeadlineExpired,
            ConnectivityError::Stream(stream()),
            ConnectivityError::Shared(std::sync::Arc::new(ConnectivityError::Stream(stream()))),
        ] {
            assert!(error.is_transient(), "{}", error.class());
        }
        for error in [
            ConnectivityError::Denied,
            ConnectivityError::Invalidated(None),
            ConnectivityError::Stopped,
            ConnectivityError::Offline,
            ConnectivityError::AddressMissing,
            ConnectivityError::ExchangeAlreadyUsed,
            ConnectivityError::Capability(StreamCapabilityRefusal::UnidirectionalUnavailable),
            ConnectivityError::AddressLookup(stream()),
            ConnectivityError::Admission(stream()),
        ] {
            assert!(!error.is_transient(), "{}", error.class());
        }
    }

    /// 撤销快照失效后读取成员权威失败：保留来源链，同时仍按请求失效分类（可恢复、非拒绝）。
    #[test]
    fn invalidation_keeps_the_admission_read_source() {
        let error = ConnectivityError::Invalidated(Some(Box::new(std::io::Error::other(
            "membership read failed",
        ))));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.is_unavailable());
        assert!(!error.is_denied());
        assert!(!error.is_transient());
        assert!(std::error::Error::source(&ConnectivityError::Invalidated(None)).is_none());
    }

    /// 授权拒绝可穿过共享拨号结果识别；请求失效不是拒绝。两者都不改变既有不可用判断。
    #[test]
    fn denial_is_recognized_through_shared_results() {
        let shared = ConnectivityError::Shared(std::sync::Arc::new(ConnectivityError::Shared(
            std::sync::Arc::new(ConnectivityError::Denied),
        )));
        assert!(shared.is_denied());
        assert!(shared.is_unavailable());
        for error in [
            ConnectivityError::Offline,
            ConnectivityError::AddressMissing,
            ConnectivityError::Invalidated(None),
            ConnectivityError::Shared(std::sync::Arc::new(ConnectivityError::Invalidated(None))),
            ConnectivityError::Backend(Box::new(std::io::Error::other("unreachable"))),
            ConnectivityError::Shared(std::sync::Arc::new(ConnectivityError::Offline)),
        ] {
            assert!(!error.is_denied(), "{}", error.class());
            assert!(error.is_unavailable(), "{}", error.class());
        }
    }
}
