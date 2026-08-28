use async_trait::async_trait;
use uc_core::membership::{JoinerCompletePreparation, SpaceAdmissionEnvelopeV1, SpaceAdmissionId};

use super::{PrepareJoinerActivationError, PreparedJoinerActivation};

#[async_trait]
pub trait PrepareJoinerActivationPort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: JoinerCompletePreparation<'_>,
        complete: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerActivation, PrepareJoinerActivationError>;
}
