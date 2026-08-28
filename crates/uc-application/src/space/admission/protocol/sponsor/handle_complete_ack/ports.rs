use async_trait::async_trait;
use uc_core::membership::{
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorSettlementPreparation,
};

use super::{PrepareSponsorSettledError, PreparedSponsorSettled};

#[async_trait]
pub trait PrepareSponsorSettledPort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorSettlementPreparation<'_>,
        complete_ack: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorSettled, PrepareSponsorSettledError>;
}
