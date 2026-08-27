use async_trait::async_trait;
use uc_core::membership::{SpaceAdmissionId, SponsorCandidatePreparation};

use super::{
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission, LoadedSponsorJoinRequest,
    PreparedSponsorCandidate, SpaceAdmissionMessageReply, SponsorAdmissionMutation,
    SponsorJoinRequestCommitToken,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SponsorJoinRequestStateError {
    #[error("sponsor join request state is locked")]
    Locked,
    #[error("sponsor join request state changed")]
    StateChanged,
    #[error("sponsor join request state requires recovery")]
    RecoveryRequired,
    #[error("sponsor join request state is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait SponsorJoinRequestStatePort: Send + Sync {
    async fn load(
        &self,
        message: &AuthenticatedSpaceAdmissionMessage,
    ) -> Result<LoadedSponsorJoinRequest, SponsorJoinRequestStateError>;

    async fn commit(
        &self,
        token: SponsorJoinRequestCommitToken,
        mutation: SponsorAdmissionMutation,
    ) -> Result<CommittedSponsorAdmission, SponsorJoinRequestStateError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrepareSponsorCandidateError {
    #[error("sponsor candidate material is invalid")]
    Invalid,
    #[error("sponsor candidate material is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait PrepareSponsorCandidatePort: Send + Sync {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HandleAuthenticatedSpaceAdmissionMessageError {
    #[error("the authenticated admission message is invalid")]
    Invalid,
    #[error("the authenticated admission message conflicts with saved state")]
    Conflict,
    #[error("the authenticated admission message is out of order")]
    OutOfOrder,
    #[error("space admission state is locked")]
    Locked,
    #[error("space admission state changed")]
    StateChanged,
    #[error("space admission requires recovery")]
    RecoveryRequired,
    #[error("space admission is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait HandleAuthenticatedSpaceAdmissionMessagePort: Send + Sync {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError>;
}
