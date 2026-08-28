use async_trait::async_trait;
use uc_core::membership::{SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorCompletePreparation};

use super::{PrepareSponsorCompleteError, PreparedSponsorComplete};

#[async_trait]
pub trait PrepareSponsorCompletePort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCompletePreparation<'_>,
        applied: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorComplete, PrepareSponsorCompleteError>;
}
