use super::model::{
    JoinerStartMaterial, JoinerStartMutation, LoadedJoinerStartState, PreparedJoinerInvitation,
    SpaceAdmissionCommitToken,
};

#[derive(Debug, thiserror::Error)]
pub enum PrepareJoinerInvitationError {
    #[error("the invitation is invalid")]
    Invalid,
    #[error("invitation preparation is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareJoinerInvitationError {
    pub fn unavailable(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}

impl From<PrepareJoinerInvitationError> for JoinSpaceError {
    fn from(error: PrepareJoinerInvitationError) -> Self {
        match error {
            PrepareJoinerInvitationError::Invalid => Self::InvalidInvitation,
            PrepareJoinerInvitationError::Unavailable { .. } => Self::Unavailable,
        }
    }
}

#[async_trait]
pub trait PrepareJoinerInvitationPort: Send + Sync {
    async fn prepare(
        &self,
        input: &JoinSpaceInput,
    ) -> Result<PreparedJoinerInvitation, PrepareJoinerInvitationError>;
}
use crate::space::admission::{JoinSpaceError, JoinSpaceInput};
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum JoinerStartMaterialError {
    #[error("the invitation cannot start a new admission")]
    InvalidInvitation,

    #[error("joiner start material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl JoinerStartMaterialError {
    pub fn unavailable(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}

impl From<JoinerStartMaterialError> for JoinSpaceError {
    fn from(error: JoinerStartMaterialError) -> Self {
        match error {
            JoinerStartMaterialError::InvalidInvitation => Self::InvalidInvitation,
            JoinerStartMaterialError::Unavailable { .. } => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JoinerStartStateError {
    #[error("joiner start state is locked")]
    Locked,

    #[error("joiner start state changed")]
    StateChanged,

    #[error("joiner start state requires recovery")]
    RecoveryRequired,

    #[error("joiner start state is unavailable")]
    Unavailable,
}

impl From<JoinerStartStateError> for JoinSpaceError {
    fn from(error: JoinerStartStateError) -> Self {
        match error {
            JoinerStartStateError::Locked => Self::Locked,
            JoinerStartStateError::StateChanged => Self::StateChanged,
            JoinerStartStateError::RecoveryRequired => Self::RecoveryRequired,
            JoinerStartStateError::Unavailable => Self::Unavailable,
        }
    }
}

#[async_trait]
pub trait JoinerStartMaterialPort: Send + Sync {
    async fn create(
        &self,
        input: &JoinSpaceInput,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError>;
}

#[async_trait]
pub trait JoinerStartStatePort: Send + Sync {
    /// 一次性取得开始加入所需的完整视图
    async fn load(&self) -> Result<LoadedJoinerStartState, JoinerStartStateError>;

    /// 交回凭证，并一次保存 Core 产生的完整变化
    async fn commit(
        &self,
        token: SpaceAdmissionCommitToken,
        mutation: JoinerStartMutation,
    ) -> Result<(), JoinerStartStateError>;
}
