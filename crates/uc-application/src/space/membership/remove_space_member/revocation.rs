use async_trait::async_trait;

use super::{
    AdmissionAbandonmentRevocationTarget, AdmissionRevocationResult, AdmissionRevocationTarget,
    RemoveSpaceMemberError,
};

#[async_trait]
pub trait AdmissionRevocationPort: Send + Sync {
    async fn revoke_admission(
        &self,
        target: AdmissionRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError>;

    async fn revoke_abandoned_admission(
        &self,
        _target: AdmissionAbandonmentRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        Err(RemoveSpaceMemberError::Unavailable)
    }
}
