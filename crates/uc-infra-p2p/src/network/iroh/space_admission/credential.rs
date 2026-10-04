use async_trait::async_trait;
use uc_core::membership::{AdmissionContinuationCredential, InvitationId, SpaceAdmissionId};

use uc_infra_crypto::space_admission_auth::{
    SpaceAdmissionRegistration, SpaceAdmissionServerSetup,
};

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

    pub fn into_parts(self) -> (SpaceAdmissionServerSetup, SpaceAdmissionRegistration) {
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

impl SpaceAdmissionChannelCredentialError {
    /// 协议负责人只取得脱敏分类；具体来源类型留在构造它的 Infra 适配器内部，
    /// 跨 crate 不再对 `source` 做向下转型。
    pub fn diagnostic_failure(
        &self,
    ) -> uc_observability_contract::diagnostics::connectivity::CredentialFailure {
        use uc_observability_contract::diagnostics::connectivity::CredentialFailure;
        match self {
            Self::Unavailable { .. } => CredentialFailure::Unavailable,
            Self::Rejected { .. } => CredentialFailure::RecoveryRequired,
        }
    }
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
