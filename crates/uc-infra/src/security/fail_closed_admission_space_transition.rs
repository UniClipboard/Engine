use async_trait::async_trait;
use uc_application::deps::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort, CurrentSpaceIdentityError,
    DeviceManagementResetDataPort, InitialSpaceActivationPort,
};
use uc_core::ids::SpaceId;
use uc_core::membership::{AdmissionSpaceTransitionV2, MembershipBranchTransitionV1};

/// Rejects activation until the profile has a real generation-switch adapter.
pub struct FailClosedAdmissionSpaceTransition;

#[async_trait]
impl AdmissionSpaceTransitionPort for FailClosedAdmissionSpaceTransition {
    async fn preflight_source_history(
        &self,
        _preserve_unreadable_history: bool,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Unavailable)
    }

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

#[async_trait]
impl DeviceManagementResetDataPort for FailClosedAdmissionSpaceTransition {
    async fn prepare_device_management_reset(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Unavailable)
    }

    async fn stage_device_management_reset_mutations(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::RecoveryRequired)
    }

    async fn promote_device_management_reset(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::RecoveryRequired)
    }

    async fn finalize_device_management_reset(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::RecoveryRequired)
    }
}

#[async_trait]
impl InitialSpaceActivationPort for FailClosedAdmissionSpaceTransition {
    async fn activate_initial_space(
        &self,
        _space_id: &SpaceId,
    ) -> Result<(), CurrentSpaceIdentityError> {
        Err(CurrentSpaceIdentityError::Unavailable)
    }
}

#[async_trait]
impl AdvanceMembershipBranchTransitionPort for FailClosedAdmissionSpaceTransition {
    async fn advance_membership_branch_transition(
        &self,
        _input: AdvanceMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, AdvanceMembershipBranchTransitionError> {
        Err(AdvanceMembershipBranchTransitionError::Unavailable {
            source: anyhow::anyhow!("V3 Space control transition is not assembled"),
        })
    }
}
