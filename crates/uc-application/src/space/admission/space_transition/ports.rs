use async_trait::async_trait;
use thiserror::Error;

use crate::error::anyhow_error_constructor;
use crate::space::admission::protocol::JoinerActivationIntent;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionSecurityCommitmentV1, AdmissionSpaceTransitionResultV2,
    AdmissionSpaceTransitionV2, PendingGroupUpdate,
};

/// Stable classification of an admission Space transition failure.
///
/// Variants that report a failed dependency, storage or recovery capability keep
/// that failure as their source; callers must classify the chain instead of
/// dropping it. Variants that express a pure judgement (`Locked`,
/// `UnreadableHistoryRequiresConfirmation`, `InsufficientStorage`) intentionally
/// carry no source.
#[derive(Debug, Error)]
pub enum AdmissionSpaceTransitionError {
    #[error("unreadable history requires explicit confirmation")]
    UnreadableHistoryRequiresConfirmation,
    #[error("profile is locked")]
    Locked,
    #[error("insufficient storage for space transition")]
    InsufficientStorage,
    #[error("space transition is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("space transition storage failed")]
    Storage {
        #[source]
        source: anyhow::Error,
    },
    #[error("space transition state is inconsistent")]
    Inconsistent {
        #[source]
        source: anyhow::Error,
    },
    #[error("space transition requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
}

impl AdmissionSpaceTransitionError {
    anyhow_error_constructor!(pub unavailable, Unavailable);
    anyhow_error_constructor!(pub storage, Storage);
    anyhow_error_constructor!(pub inconsistent, Inconsistent);
    anyhow_error_constructor!(pub recovery_required, RecoveryRequired);

    /// State a transition step requires is absent, so the transition cannot
    /// continue. `context` names the missing material or checkpoint.
    pub fn missing(context: &'static str) -> Self {
        Self::inconsistent(anyhow::anyhow!(context))
    }
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

    async fn advance_admission(
        &self,
        transition: &AdmissionSpaceTransitionV2,
        intent: JoinerActivationIntent,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        if transition.attempt_id() != intent.admission_id() {
            return Err(AdmissionSpaceTransitionError::missing(
                "activation intent does not belong to the transition attempt",
            ));
        }
        self.advance(transition).await
    }

    async fn discard_pre_activation(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError>;

    async fn terminate_admission(
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
