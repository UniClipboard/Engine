use async_trait::async_trait;
use uc_core::membership::{JoinerActivationPreparation, SpaceAdmissionId};

use super::{
    CompletedJoinerActivation, ExecuteJoinerActivationError, JoinerActivationCommitToken,
    JoinerActivationMutation, JoinerActivationStateError, LoadedJoinerActivation,
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
}
