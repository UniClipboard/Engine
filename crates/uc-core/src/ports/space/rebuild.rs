use async_trait::async_trait;
use thiserror::Error;

use crate::ids::SpaceId;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceRebuildTransitionError {
    #[error("space rebuild data transition is unavailable")]
    Unavailable,

    #[error("space rebuild data transition storage failed")]
    Storage,

    #[error("insufficient storage for space rebuild")]
    InsufficientStorage,

    #[error("space rebuild data transition is inconsistent")]
    Inconsistent,

    #[error("space rebuild data transition requires recovery")]
    RecoveryRequired,
}

#[async_trait]
pub trait SpaceRebuildTransitionPort: Send + Sync {
    /// 为指定 Space 记录待完成的数据切换。
    ///
    /// 对相同 Space ID 重复调用必须保留既有待完成状态。
    async fn prepare(&self) -> Result<SpaceId, SpaceRebuildTransitionError>;

    /// 将重建产生的数据变更写入指定 Space 的隔离目标状态。
    ///
    /// 成功后，目标状态尚未生效为当前状态。
    async fn stage(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError>;

    /// 将已暂存的目标状态生效为当前状态。
    ///
    /// 不存在对应 Space ID 的暂存状态时返回错误。
    async fn promote(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError>;

    /// 完成目标状态生效后的收尾。
    ///
    /// 对已经完成收尾的同一 Space ID 重复调用必须成功。
    async fn finalize(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpaceRebuildAdmissionStateError {
    #[error("Space 重建准入状态不可用")]
    Unavailable,

    #[error("Space 重建准入状态不一致")]
    Inconsistent,
}

#[async_trait]
pub trait SpaceRebuildAdmissionStatePort: Send + Sync {
    /// 清除属于先前 Space 的本机准入状态。
    ///
    /// 成功后，不保留会将本机识别为正在加入或等待加入先前 Space 的准入记录。
    /// 重复调用必须成功。
    async fn clear_prior_space_admission_state(
        &self,
    ) -> Result<(), SpaceRebuildAdmissionStateError>;
}
