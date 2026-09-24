use crate::runtime_lifecycle::LifecycleError;

#[derive(Debug, thiserror::Error)]
pub enum ProfileFactoryResetError {
    #[error(transparent)]
    Lifecycle(#[from] ProfileLifecycleError),
    #[error(transparent)]
    Repository(#[from] ProfileLifecycleRepositoryError),
    #[error("profile lifecycle state is missing")]
    LifecycleMissing,
    #[error("profile runtime could not be stopped")]
    StopRuntime {
        #[source]
        source: LifecycleError,
    },
    #[error("profile keys could not be wiped")]
    WipeKeys,
    #[error("profile state could not be cleared")]
    ClearState,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProfileLifecycleError {
    #[error("profile generation does not match the active lifecycle")]
    GenerationConflict,
    #[error("profile lifecycle transition is not allowed from the current state")]
    InvalidTransition,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileLifecycleRepositoryError {
    #[error("profile lifecycle storage is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("profile lifecycle record is corrupt")]
    Corrupt {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("profile lifecycle record changed before it could be saved")]
    Conflict,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl ProfileLifecycleRepositoryError {
    pub fn corrupt() -> Self {
        Self::Corrupt { source: None }
    }

    pub fn corrupt_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Corrupt {
            source: Some(source.into()),
        }
    }

    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: Some(source.into()),
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("profile factory reset capability failed")]
pub struct ProfileFactoryResetCapabilityError;
