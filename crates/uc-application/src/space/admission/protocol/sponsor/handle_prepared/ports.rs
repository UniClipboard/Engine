use async_trait::async_trait;
use uc_core::membership::{SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorCommitPreparation};

use super::{PrepareSponsorCommitError, PreparedSponsorCommit};

#[async_trait]
pub trait PrepareSponsorCommitPort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCommitPreparation<'_>,
        prepared: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorCommit, PrepareSponsorCommitError>;
}
