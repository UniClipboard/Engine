use super::error::SpaceMembershipStateRepositoryError;
use async_trait::async_trait;
use uc_core::membership::SpaceMembershipState;

/// 完整 Space 成员状态的加密持久化接口。
///
/// 实现必须拒绝属于其他 Space 的状态，保证已保存变化在重启后顺序不变，
/// 并且不得向 application 暴露底层存储错误详情。
#[async_trait]
pub trait SpaceMembershipStateRepositoryPort: Send + Sync {
    async fn save_state(
        &self,
        state: &SpaceMembershipState,
    ) -> Result<(), SpaceMembershipStateRepositoryError>;

    async fn load_state(
        &self,
    ) -> Result<Option<SpaceMembershipState>, SpaceMembershipStateRepositoryError>;
}
