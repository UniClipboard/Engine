use super::error::{
    SpaceMembershipRebuildError, SpaceRebuildProgressError, SpaceRebuildTransitionError,
    SpaceSessionRebindError,
};
use async_trait::async_trait;
use uc_core::ids::SpaceId;
use uc_core::SpaceMember;

pub(crate) struct SpaceRebuildPreparation {
    pub(crate) space_id: SpaceId,
    pub(crate) already_committed: bool,
}

/// 将成员状态重建为仅包含 `local_member` 的完整基线
///
/// 成功后，除 `local_member` 外的成员和成员关系均不再保留；
/// `local_member` 已被保存，成员资格基线可用于后续准入。
///
/// 对相同成员重复调用必须成功。
#[async_trait]
pub trait SpaceMembershipRebuildPort: Send + Sync {
    async fn rebuild(&self, local_member: &SpaceMember) -> Result<(), SpaceMembershipRebuildError>;
}

#[async_trait]
pub(crate) trait SpaceMembershipResetPort: Send + Sync {
    async fn reset(&self) -> Result<(), SpaceMembershipRebuildError>;
}

#[async_trait]
pub trait RebindSpaceSessionPort: Send + Sync {
    async fn rebind_to_space(&self, space_id: &SpaceId) -> Result<(), SpaceSessionRebindError>;
}

#[async_trait]
pub trait SpaceRebuildProgressPort: Send + Sync {
    async fn load_target(&self) -> Result<Option<SpaceId>, SpaceRebuildProgressError>;
    async fn store_target(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildProgressError>;
    async fn clear_target(&self) -> Result<(), SpaceRebuildProgressError>;
}

#[async_trait]
pub trait SpaceRebuildTransitionPort: Send + Sync {
    /// 创建或恢复当前 profile 唯一尚未完成的 Space 重建目标。
    ///
    /// 首次调用必须先持久保存新目标，再返回其 Space ID。重启或重试时，
    /// 若存在尚未完成的目标，必须返回同一 Space ID，不得创建第二个目标。
    async fn prepare(&self) -> Result<SpaceRebuildPreparation, SpaceRebuildTransitionError>;

    /// 将重建产生的数据变更写入指定 Space 的隔离目标状态。
    ///
    /// 成功后，目标状态尚未生效为当前状态。若该目标已经是当前活动
    /// Space，重复调用必须直接成功。
    async fn stage(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError>;

    /// 将已暂存的目标状态生效为当前状态。
    ///
    /// 不存在对应 Space ID 的暂存状态时返回错误；该目标已经生效时，
    /// 重复调用必须直接成功。
    async fn promote(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError>;

    /// 完成目标状态生效后的收尾。
    ///
    /// 对已经完成收尾的同一 Space ID 重复调用必须成功。
    async fn finalize(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError>;
}
