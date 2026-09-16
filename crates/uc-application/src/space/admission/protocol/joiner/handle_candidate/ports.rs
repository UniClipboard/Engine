use async_trait::async_trait;
use uc_core::membership::{JoinerCandidatePreparation, SpaceAdmissionEnvelopeV1};

use super::PreparedJoinerCandidateMaterial;

#[derive(Debug, thiserror::Error)]
pub enum PrepareJoinerCandidateError {
    #[error("joiner candidate material is invalid")]
    Invalid,
    #[error("joiner candidate material is invalid")]
    InvalidSource {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner candidate material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareJoinerCandidateError {
    pub fn invalid<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::InvalidSource {
            source: source.into(),
        }
    }

    pub fn unavailable<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}

#[async_trait]
pub trait PrepareJoinerCandidatePort: Send + Sync {
    async fn prepare(
        &self,
        preparation: JoinerCandidatePreparation<'_>,
        candidate: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError>;
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::PrepareJoinerCandidateError;

    #[test]
    fn dependency_failure_keeps_stable_classification_and_source() {
        let error = PrepareJoinerCandidateError::invalid(std::io::Error::other("decode failed"));

        assert!(matches!(
            error,
            PrepareJoinerCandidateError::InvalidSource { .. }
        ));
        assert!(error.source().is_some());
    }
}
