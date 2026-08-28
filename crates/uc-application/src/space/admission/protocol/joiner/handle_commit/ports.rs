use async_trait::async_trait;
use uc_core::membership::{JoinerAppliedPreparation, SpaceAdmissionId};

use super::{PrepareJoinerAppliedError, PreparedJoinerAppliedMaterial};

#[async_trait]
pub trait PrepareJoinerAppliedPort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: JoinerAppliedPreparation<'_>,
    ) -> Result<PreparedJoinerAppliedMaterial, PrepareJoinerAppliedError>;
}
