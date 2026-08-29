use async_trait::async_trait;
use uc_core::membership::JoinId;

use super::{
    JoinerCancellationCommitToken, JoinerCancellationMaterial, JoinerCancellationMaterialError,
    JoinerCancellationMutation, JoinerCancellationStateError, LoadedCurrentJoin,
};

#[async_trait]
pub trait CurrentJoinAdmissionStatePort: Send + Sync {
    /// 只读取与调用方给定 JoinId 一致的当前本机加入，并绑定条件提交凭证。
    async fn load(
        &self,
        join_id: JoinId,
    ) -> Result<Option<LoadedCurrentJoin>, JoinerCancellationStateError>;

    /// 原子验证读取凭证并保存完整领域变化。
    async fn commit(
        &self,
        token: JoinerCancellationCommitToken,
        mutation: JoinerCancellationMutation,
    ) -> Result<(), JoinerCancellationStateError>;
}

#[async_trait]
pub trait PrepareJoinerCancellationPort: Send + Sync {
    async fn prepare(&self) -> Result<JoinerCancellationMaterial, JoinerCancellationMaterialError>;
}
