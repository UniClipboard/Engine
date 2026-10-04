use async_trait::async_trait;
use uc_core::membership::{AdmissionContinuationCredential, InvitationId, SpaceAdmissionId};

use uc_infra_crypto::space_admission_auth::{
    SpaceAdmissionRegistration, SpaceAdmissionServerSetup,
};
use uc_observability_contract::diagnostics::connectivity::CredentialFailure;

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
        failure: CredentialFailure,
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission channel credential was rejected")]
    Rejected {
        failure: CredentialFailure,
        #[source]
        source: anyhow::Error,
    },
}

impl SpaceAdmissionChannelCredentialError {
    /// 协议负责人只取得脱敏分类。分类由构造错误的凭据负责人按其来源类型写入，
    /// 网络侧不对 `source` 做跨 crate 向下转型。
    pub fn diagnostic_failure(&self) -> CredentialFailure {
        match self {
            Self::Unavailable { failure, .. } | Self::Rejected { failure, .. } => *failure,
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
