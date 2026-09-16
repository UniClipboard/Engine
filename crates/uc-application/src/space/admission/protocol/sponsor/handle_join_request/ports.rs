use async_trait::async_trait;
use uc_core::membership::{SpaceAdmissionId, SponsorCandidatePreparation};

use super::{PrepareSponsorCandidateError, PreparedSponsorCandidate};

#[async_trait]
pub trait PrepareSponsorCandidatePort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError>;
}
