use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use anyhow::Error as SourceError;
use thiserror::Error;
use tokio::task::JoinError;

/// 网络重建失败的稳定分类，供宿主展示；不携带端口或地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRecoveryFailure {
    /// 固定监听端口已被占用，需要用户释放或更换端口，自动重试没有意义。
    ListenPortUnavailable,
    Other,
}

#[derive(Clone)]
pub struct RebuildNetworkSessionError {
    retryable: bool,
    failure: NetworkRecoveryFailure,
    source: Arc<SourceError>,
}

impl RebuildNetworkSessionError {
    pub fn new(source: impl Into<SourceError>, retryable: bool) -> Self {
        Self {
            source: Arc::new(source.into()),
            retryable,
            failure: NetworkRecoveryFailure::Other,
        }
    }

    /// 由构造失败的一方（Engine）标明分类；未标明时为 `Other`。
    pub fn with_failure(mut self, failure: NetworkRecoveryFailure) -> Self {
        self.failure = failure;
        self
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub fn failure(&self) -> NetworkRecoveryFailure {
        self.failure
    }
}

impl fmt::Display for RebuildNetworkSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("network session rebuild failed")
    }
}

impl fmt::Debug for RebuildNetworkSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RebuildNetworkSessionError")
            .field("retryable", &self.retryable)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl StdError for RebuildNetworkSessionError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref().as_ref())
    }
}

impl PartialEq for RebuildNetworkSessionError {
    fn eq(&self, other: &Self) -> bool {
        self.retryable == other.retryable
            && self.failure == other.failure
            && Arc::ptr_eq(&self.source, &other.source)
    }
}

impl Eq for RebuildNetworkSessionError {}

#[derive(Clone, Error)]
pub enum NetworkRecoveryRequestError {
    #[error("network recovery runtime is stopped")]
    Stopped,
    #[error(transparent)]
    Rebuild(#[from] RebuildNetworkSessionError),
    #[error("network recovery worker failed")]
    Task(#[source] Arc<JoinError>),
}

impl fmt::Debug for NetworkRecoveryRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stopped => "Stopped",
            Self::Rebuild(_) => "Rebuild",
            Self::Task(_) => "Task",
        })
    }
}

impl PartialEq for NetworkRecoveryRequestError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Stopped, Self::Stopped) => true,
            (Self::Rebuild(left), Self::Rebuild(right)) => left == right,
            (Self::Task(left), Self::Task(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for NetworkRecoveryRequestError {}
