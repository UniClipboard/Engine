use async_trait::async_trait;
use uc_core::membership::{JoinerActivationPreparation, SpaceAdmissionId};

use super::{
    CompletedJoinerActivation, ExecuteJoinerActivationError, JoinerActivationCommitToken,
    JoinerActivationIntent, JoinerActivationMutation, JoinerActivationStateError,
    LoadedJoinerActivation,
};

#[async_trait]
pub trait JoinerActivationStatePort: Send + Sync {
    /// Loads the one locally saved activation that still needs execution.
    async fn load(&self) -> Result<Option<LoadedJoinerActivation>, JoinerActivationStateError>;

    /// Atomically saves the replacement aggregate and all declared effects.
    async fn commit(
        &self,
        token: JoinerActivationCommitToken,
        mutation: JoinerActivationMutation,
    ) -> Result<(), JoinerActivationStateError>;
}

#[async_trait]
pub trait ExecuteJoinerActivationPort: Send + Sync {
    /// Executes the exact saved activation plan and returns its durable result.
    /// Repeating the same plan after an uncertain commit must return the same result.
    async fn execute(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError>;

    /// 隔离终止记录保留的准确本机目标；重复执行必须安全。
    async fn terminate(
        &self,
        admission_id: SpaceAdmissionId,
        saved_transition: &[u8],
    ) -> Result<(), ExecuteJoinerActivationError>;
}

#[async_trait]
pub trait ValidateJoinerActivationIntentPort: Send + Sync {
    /// 只认可当前仍处于 Activating 且保存了同一计划的准入意图。
    async fn validate(
        &self,
        intent: JoinerActivationIntent,
    ) -> Result<bool, JoinerActivationStateError>;
}
