use thiserror::Error;

use crate::error::anyhow_error_constructor;

#[derive(Debug, Error)]
pub(crate) enum RebuildSpaceError {
    #[error("failed to prepare single-device space rebuild")]
    PreparationFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to stage single-device space rebuild")]
    StagingFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to rebuild the single-device space")]
    RebuildFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to commit single-device space rebuild")]
    CommitFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("single-device space rebuild committed but finalization failed")]
    FinalizationFailed {
        #[source]
        source: anyhow::Error,
    },

    #[error("local device name is unavailable")]
    DeviceNameUnavailable,

    #[error("clock returned an invalid timestamp")]
    InvalidClock,
}

impl RebuildSpaceError {
    anyhow_error_constructor!(preparation, PreparationFailed);
    anyhow_error_constructor!(staging, StagingFailed);
    anyhow_error_constructor!(rebuild, RebuildFailed);
    anyhow_error_constructor!(commit, CommitFailed);
    anyhow_error_constructor!(finalize, FinalizationFailed);

    /// 数据转换暂时不可用（锁争用、转换租约被占用）：持久状态可续做，稍后重试即可继续。
    pub(crate) fn is_temporarily_unavailable(&self) -> bool {
        match self {
            Self::PreparationFailed { source }
            | Self::StagingFailed { source }
            | Self::CommitFailed { source }
            | Self::FinalizationFailed { source } => matches!(
                source.downcast_ref::<SpaceRebuildTransitionError>(),
                Some(SpaceRebuildTransitionError::Unavailable { .. })
            ),
            Self::RebuildFailed { .. } | Self::DeviceNameUnavailable | Self::InvalidClock => false,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceMembershipRebuildError {
    #[error("space membership rebuild is unavailable")]
    Unavailable,

    #[error("space membership rebuild state is inconsistent")]
    Inconsistent,
}

#[derive(Debug, Error)]
pub enum SpaceRebuildTransitionError {
    #[error("space rebuild data transition is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },

    #[error("space rebuild data transition storage failed")]
    Storage {
        #[source]
        source: anyhow::Error,
    },

    #[error("insufficient storage for space rebuild")]
    InsufficientStorage,

    #[error("space rebuild data transition is inconsistent")]
    Inconsistent {
        #[source]
        source: anyhow::Error,
    },

    #[error("space rebuild data transition requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
}

impl SpaceRebuildTransitionError {
    anyhow_error_constructor!(unavailable, Unavailable);
    anyhow_error_constructor!(storage, Storage);
    anyhow_error_constructor!(inconsistent, Inconsistent);
    anyhow_error_constructor!(recovery_required, RecoveryRequired);
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceSessionRebindError {
    #[error("space session rebind is unavailable")]
    Unavailable,

    #[error("space session rebind state is inconsistent")]
    Inconsistent,
}

#[derive(Debug, Error)]
pub enum SpaceRebuildProgressError {
    #[error("space rebuild progress storage is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },

    #[error("space rebuild progress is inconsistent")]
    Inconsistent,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl SpaceRebuildProgressError {
    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: Some(source.into()),
        }
    }
}
