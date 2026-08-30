use async_trait::async_trait;

use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionSecurityCommitmentV1, AdmissionSpaceTransitionResultV2,
    AdmissionSpaceTransitionV2, PendingGroupUpdate,
};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AdmissionSpaceTransitionError {
    #[error("unreadable history requires explicit confirmation")]
    UnreadableHistoryRequiresConfirmation,
    #[error("profile is locked")]
    Locked,
    #[error("space transition is unavailable")]
    Unavailable,
    #[error("space transition storage failed")]
    Storage,
    #[error("insufficient storage for space transition")]
    InsufficientStorage,
    #[error("space transition state is inconsistent")]
    Inconsistent,
    #[error("space transition requires recovery")]
    RecoveryRequired,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionSpaceTransitionPreparationV2 {
    pub attempt_id: uc_core::membership::SpaceAdmissionId,
    pub target_space_id: String,
    pub target_security_commitment: AdmissionSecurityCommitmentV1,
    pub target_membership_history: Vec<u8>,
    pub target_security_state: Vec<u8>,
    pub target_protection_group_id: String,
    pub target_key_catalog: Vec<u8>,
    pub local_device_id: DeviceId,
    pub target_relationships: Vec<AdmissionChangeFacts>,
    pub relayed_group_updates: Vec<PendingGroupUpdate>,
    pub target_access_state: Vec<u8>,
    /// 已用本次加入口令派生、等待写入目标 generation 的 OPAQUE 服务端凭据。
    pub target_admission_credentials: Vec<u8>,
    pub preserve_unreadable_history: bool,
}

impl std::fmt::Debug for AdmissionSpaceTransitionPreparationV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionSpaceTransitionPreparationV2")
            .field("attempt_id", &self.attempt_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionSpaceTransitionStepV2 {
    Advanced(AdmissionSpaceTransitionV2),
    Finished(AdmissionSpaceTransitionResultV2),
}

#[async_trait]
pub trait AdmissionSpaceTransitionPort: Send + Sync {
    async fn preflight_source_history(
        &self,
        _preserve_unreadable_history: bool,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Ok(())
    }

    async fn prepare_if_needed(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError>;

    async fn advance(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError>;

    async fn discard_pre_activation(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError>;
}

#[async_trait]
pub trait DeviceManagementResetDataPort: Send + Sync {
    async fn prepare_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError>;

    async fn stage_device_management_reset_mutations(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError>;

    async fn promote_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError>;

    async fn finalize_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError>;
}
