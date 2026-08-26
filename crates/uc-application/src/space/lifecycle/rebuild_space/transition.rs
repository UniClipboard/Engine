use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::SpaceId;
use uc_core::membership::{SpaceSecurityStateResetError, SpaceSecurityStateResetPort};

use super::error::SpaceRebuildTransitionError;
use super::ports::{SpaceRebuildPreparation, SpaceRebuildProgressPort, SpaceRebuildTransitionPort};
use crate::deps::{AdmissionSpaceTransitionError, DeviceManagementResetDataPort};
use crate::space::lifecycle::CurrentSpaceIdentityPort;
use crate::space::membership::RePairingState;

pub(crate) struct SpaceRebuildTransition {
    data: Arc<dyn DeviceManagementResetDataPort>,
    security: Arc<dyn SpaceSecurityStateResetPort>,
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    progress: Arc<dyn SpaceRebuildProgressPort>,
    re_pairing_state: Arc<RePairingState>,
}

impl SpaceRebuildTransition {
    pub(crate) fn new(
        data: Arc<dyn DeviceManagementResetDataPort>,
        security: Arc<dyn SpaceSecurityStateResetPort>,
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        progress: Arc<dyn SpaceRebuildProgressPort>,
        re_pairing_state: Arc<RePairingState>,
    ) -> Self {
        Self {
            data,
            security,
            current_space_identity,
            progress,
            re_pairing_state,
        }
    }
}

#[async_trait]
impl SpaceRebuildTransitionPort for SpaceRebuildTransition {
    async fn prepare(&self) -> Result<SpaceRebuildPreparation, SpaceRebuildTransitionError> {
        let current_space_id = self
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|_| SpaceRebuildTransitionError::Storage)?;
        let pending = self
            .progress
            .load_target()
            .await
            .map_err(|_| SpaceRebuildTransitionError::Storage)?;

        if let Some(space_id) = pending {
            let already_committed = current_space_id.as_ref() == Some(&space_id);
            if !already_committed {
                self.data
                    .prepare_device_management_reset(&space_id)
                    .await
                    .map_err(map_data_error)?;
            }
            return Ok(SpaceRebuildPreparation {
                space_id,
                already_committed,
            });
        }

        let space_id = SpaceId::new();
        self.progress
            .store_target(&space_id)
            .await
            .map_err(|_| SpaceRebuildTransitionError::Storage)?;
        self.data
            .prepare_device_management_reset(&space_id)
            .await
            .map_err(map_data_error)?;

        Ok(SpaceRebuildPreparation {
            space_id,
            already_committed: false,
        })
    }

    async fn stage(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError> {
        self.data
            .stage_device_management_reset_mutations(space_id)
            .await
            .map_err(map_data_error)
    }

    async fn promote(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError> {
        self.data
            .promote_device_management_reset(space_id)
            .await
            .map_err(map_data_error)
    }

    async fn finalize(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildTransitionError> {
        self.security
            .clear_space_security_state_except(space_id)
            .await
            .map_err(map_security_error)?;
        self.data
            .finalize_device_management_reset(space_id)
            .await
            .map_err(map_data_error)?;
        self.re_pairing_state
            .require_after_relationship_reset()
            .await
            .map_err(|_| SpaceRebuildTransitionError::Storage)?;
        self.progress
            .clear_target()
            .await
            .map_err(|_| SpaceRebuildTransitionError::Storage)
    }
}

fn map_data_error(error: AdmissionSpaceTransitionError) -> SpaceRebuildTransitionError {
    match error {
        AdmissionSpaceTransitionError::Locked | AdmissionSpaceTransitionError::Unavailable => {
            SpaceRebuildTransitionError::Unavailable
        }
        AdmissionSpaceTransitionError::Storage => SpaceRebuildTransitionError::Storage,
        AdmissionSpaceTransitionError::InsufficientStorage => {
            SpaceRebuildTransitionError::InsufficientStorage
        }
        AdmissionSpaceTransitionError::Inconsistent
        | AdmissionSpaceTransitionError::UnreadableHistoryRequiresConfirmation => {
            SpaceRebuildTransitionError::Inconsistent
        }
        AdmissionSpaceTransitionError::RecoveryRequired => {
            SpaceRebuildTransitionError::RecoveryRequired
        }
    }
}

fn map_security_error(_error: SpaceSecurityStateResetError) -> SpaceRebuildTransitionError {
    SpaceRebuildTransitionError::Storage
}
