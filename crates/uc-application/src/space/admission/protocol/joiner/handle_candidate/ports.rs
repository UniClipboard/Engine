use async_trait::async_trait;
use uc_core::membership::{JoinerCandidatePreparation, SpaceAdmissionEnvelopeV1};

use super::PreparedJoinerCandidateMaterial;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrepareJoinerCandidateError {
    #[error("joiner candidate material is invalid")]
    Invalid,
    #[error("joiner candidate material is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait PrepareJoinerCandidatePort: Send + Sync {
    async fn prepare(
        &self,
        preparation: JoinerCandidatePreparation<'_>,
        candidate: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError>;
}
