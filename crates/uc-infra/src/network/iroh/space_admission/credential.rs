use async_trait::async_trait;
use uc_core::membership::{AdmissionContinuationCredential, InvitationId, SpaceAdmissionId};

use crate::security::{SpaceAdmissionRegistration, SpaceAdmissionServerSetup};

pub struct SponsorOpaqueMaterial {
    pub(super) server_setup: SpaceAdmissionServerSetup,
    pub(super) registration: SpaceAdmissionRegistration,
}

impl SponsorOpaqueMaterial {
    pub fn new(
        server_setup: SpaceAdmissionServerSetup,
        registration: SpaceAdmissionRegistration,
    ) -> Self {
        Self {
            server_setup,
            registration,
        }
    }

    #[cfg(test)]
    pub(crate) fn into_parts(self) -> (SpaceAdmissionServerSetup, SpaceAdmissionRegistration) {
        (self.server_setup, self.registration)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceAdmissionChannelCredentialError {
    #[error("space admission channel credential is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission channel credential was rejected")]
    Rejected {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait SpaceAdmissionChannelCredentialPort: Send + Sync {
    async fn resolve_initial(
        &self,
        invitation_id: InvitationId,
        admission_id: SpaceAdmissionId,
    ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionChannelCredentialError>;

    async fn load_continuation(
        &self,
        admission_id: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, SpaceAdmissionChannelCredentialError>;
}
