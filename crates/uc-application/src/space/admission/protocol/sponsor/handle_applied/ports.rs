use async_trait::async_trait;
use uc_core::membership::{
    AdmissionActivatedSecurityState, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
    SponsorCompletePreparation, VersionedMembershipHistory,
};

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

#[derive(Debug, thiserror::Error)]
#[error("Sponsor admission activation failed")]
pub struct ActivateSponsorAdmissionError {
    #[source]
    source: anyhow::Error,
}

impl ActivateSponsorAdmissionError {
    pub fn new(source: anyhow::Error) -> Self {
        Self { source }
    }
}

#[async_trait]
pub trait ActivateSponsorAdmissionPort: Send + Sync {
    /// 激活邀请方为新成员准备的安全状态，并返回随之提交的已验证成员历史；成员事实由调用方提交。
    /// 实现必须按同一准备资料幂等。
    async fn activate(
        &self,
        activated_security: &AdmissionActivatedSecurityState,
    ) -> Result<VersionedMembershipHistory, ActivateSponsorAdmissionError>;
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::ActivateSponsorAdmissionError;

    #[test]
    fn activation_error_preserves_source() {
        let error = ActivateSponsorAdmissionError::new(anyhow::anyhow!("fixture failure"));

        assert!(error.source().is_some());
    }
}
