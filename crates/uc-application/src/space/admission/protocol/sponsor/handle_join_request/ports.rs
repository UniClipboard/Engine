use async_trait::async_trait;
use uc_core::membership::{
    AdmissionContinuationRoute, SpaceAdmissionId, SponsorCandidatePreparation,
};

use super::{PrepareSponsorCandidateError, PreparedSponsorCandidate};

#[async_trait]
pub trait PrepareSponsorCandidatePort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError>;
}

/// 为当前 Sponsor Candidate 生成不透明的继续路由。
pub trait SponsorContinuationRoutePort: Send + Sync {
    /// 每次调用采样当前地址并完成路由编码与校验。
    fn prepare(&self) -> Result<AdmissionContinuationRoute, PrepareSponsorCandidateError>;
}
