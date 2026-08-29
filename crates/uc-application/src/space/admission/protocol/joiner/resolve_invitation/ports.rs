use async_trait::async_trait;
use uc_core::membership::AdmissionShortInvitationCode;
use uc_core::pairing::invitation::FullInvitation;

#[derive(Debug, thiserror::Error)]
#[error("the one-time short invitation could not be resolved")]
pub struct ResolveJoinerInvitationError {
    #[source]
    source: anyhow::Error,
}

impl ResolveJoinerInvitationError {
    pub fn unavailable(source: impl Into<anyhow::Error>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

#[async_trait]
pub trait ResolveJoinerInvitationPort: Send + Sync {
    async fn resolve_once(
        &self,
        short_code: &AdmissionShortInvitationCode,
    ) -> Result<FullInvitation, ResolveJoinerInvitationError>;
}
