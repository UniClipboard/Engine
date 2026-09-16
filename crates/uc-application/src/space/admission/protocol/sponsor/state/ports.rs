use async_trait::async_trait;

use super::{
    CommittedSponsorAdmission, LoadedSponsorAdmission, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionStateError,
};
use crate::space::admission::protocol::AuthenticatedSpaceAdmissionMessage;

#[async_trait]
pub trait SponsorAdmissionStatePort: Send + Sync {
    async fn load(
        &self,
        message: &AuthenticatedSpaceAdmissionMessage,
    ) -> Result<LoadedSponsorAdmission, SponsorAdmissionStateError>;

    /// Commits the replacement and every declared admission effect as one
    /// durable result. Returning success after saving only the replacement is
    /// invalid.
    async fn commit(
        &self,
        token: SponsorAdmissionCommitToken,
        mutation: SponsorAdmissionMutation,
    ) -> Result<CommittedSponsorAdmission, SponsorAdmissionStateError>;
}
