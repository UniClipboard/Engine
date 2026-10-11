use thiserror::Error;

use crate::ids::DeviceId;

/// Boundary error for the membership domain.
///
/// Infrastructure adapters map their internal failures (DB, I/O, etc.)
/// into `Repository` when crossing the port boundary. Use cases surface
/// `AlreadyAdmitted` and `NotFound` based on the business semantics they
/// enforce on top of the (thin) repository port.
#[derive(Debug, Error)]
pub enum MembershipError {
    #[error("member `{0}` has already been admitted")]
    AlreadyAdmitted(DeviceId),

    #[error("member `{0}` not found")]
    NotFound(DeviceId),

    #[error("membership repository failure")]
    Repository(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, Error)]
pub enum CurrentMembershipIdentityError {
    #[error("current membership identity is unavailable")]
    Unavailable {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("current membership identity could not be loaded")]
    LoadFailed {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl CurrentMembershipIdentityError {
    pub fn load_failed() -> Self {
        Self::LoadFailed { source: None }
    }

    pub fn load_failed_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::LoadFailed {
            source: Some(Box::new(source)),
        }
    }

    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Unavailable {
            source: Some(Box::new(source)),
        }
    }
}

#[derive(Debug, Error)]
pub enum SpaceSecurityStateResetError {
    #[error("space security state reset failed")]
    Repository(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, Error)]
pub enum RelationshipStateResetError {
    #[error("relationship state reset failed")]
    Repository(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// 接收方拒绝组密钥更新的可信程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupUpdateRejection {
    /// 新协议接收方的明确裁决：更新未通过密码学、授权或顺序校验。
    ByReceiver,
    /// 旧协议接收方的无原因拒绝，或升级前遗留的拒绝记录；真实原因未知。
    Unclassified,
}

#[derive(Debug, Error)]
pub enum GroupUpdateDispatchError {
    #[error("group update recipient is offline")]
    Offline {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("group update was rejected")]
    Rejected(GroupUpdateRejection),
    /// 接收方已认证连接，但本地暂时无法应用（未就绪、存储不可用）；不是对更新内容的裁决。
    #[error("group update recipient is temporarily unable to apply it")]
    ReceiverUnavailable,
    #[error("group update transport failed")]
    Transport {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl GroupUpdateDispatchError {
    pub fn offline() -> Self {
        Self::Offline { source: None }
    }

    pub fn offline_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Offline {
            source: Some(Box::new(source)),
        }
    }

    pub fn transport() -> Self {
        Self::Transport { source: None }
    }

    pub fn transport_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Transport {
            source: Some(Box::new(source)),
        }
    }
}

#[derive(Debug, Error)]
pub enum MembershipHistoryExchangeError {
    #[error("membership history recipient is offline")]
    Offline {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("space pairing is still in progress")]
    PairingInProgress,
    #[error("membership history exchange was rejected")]
    Rejected,
    #[error("membership history exchange transport failed")]
    Transport {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl MembershipHistoryExchangeError {
    pub fn offline() -> Self {
        Self::Offline { source: None }
    }

    pub fn offline_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Offline {
            source: Some(Box::new(source)),
        }
    }
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl MembershipHistoryExchangeError {
    pub fn transport() -> Self {
        Self::Transport { source: None }
    }

    pub fn transport_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Transport {
            source: Some(Box::new(source)),
        }
    }
}

#[derive(Debug, Error)]
pub enum MembershipInitializationError {
    #[error("space membership initialization is unavailable")]
    Unavailable {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("space membership initialization state is inconsistent")]
    Inconsistent {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl MembershipInitializationError {
    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Unavailable {
            source: Some(Box::new(source)),
        }
    }

    pub fn inconsistent() -> Self {
        Self::Inconsistent { source: None }
    }

    pub fn inconsistent_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Inconsistent {
            source: Some(Box::new(source)),
        }
    }
}
