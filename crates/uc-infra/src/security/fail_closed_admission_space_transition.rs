use async_trait::async_trait;
use uc_core::membership::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    AdmissionSpaceTransitionV2,
};

/// Rejects activation until the profile has a real generation-switch adapter.
pub struct FailClosedAdmissionSpaceTransition;

#[async_trait]
impl AdmissionSpaceTransitionPort for FailClosedAdmissionSpaceTransition {
    async fn prepare_if_needed(
        &self,
        _input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Unavailable)
    }

    async fn advance(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::RecoveryRequired)
    }

    async fn discard_pre_activation(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::RecoveryRequired)
    }
}
