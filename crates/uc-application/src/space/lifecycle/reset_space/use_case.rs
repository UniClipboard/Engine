use std::sync::Arc;

use super::error::ResetSpaceError;
use super::ports::PendingSpaceInvitationResetPort;
use crate::space::lifecycle::CurrentSpaceIdentityPort;
use crate::space::lifecycle::RebuildSpaceUseCase;
use crate::space::lifecycle::SpaceRebuildProgressPort;

pub(crate) struct ResetSpaceUseCase {
    rebuild_space: Arc<RebuildSpaceUseCase>,
    pending_invitations: Arc<dyn PendingSpaceInvitationResetPort>,
}

pub(crate) struct QueryCommittedDeviceManagementResetUseCase {
    progress: Arc<dyn SpaceRebuildProgressPort>,
    current_space: Arc<dyn CurrentSpaceIdentityPort>,
}

/// 用户请求重置当前 Space 后，系统创建或恢复一个只包含本机设备的新 Space，
/// 清除旧成员关系和旧空间的安全状态。
impl ResetSpaceUseCase {
    pub(crate) fn new(
        rebuild_space: Arc<RebuildSpaceUseCase>,
        pending_invitations: Arc<dyn PendingSpaceInvitationResetPort>,
    ) -> Self {
        Self {
            rebuild_space,
            pending_invitations,
        }
    }

    pub(crate) async fn execute(&self) -> Result<(), ResetSpaceError> {
        self.pending_invitations.cancel_all().await;
        self.rebuild_space
            .execute()
            .await
            .map_err(ResetSpaceError::from)?;

        Ok(())
    }
}

impl QueryCommittedDeviceManagementResetUseCase {
    pub(crate) fn new(
        progress: Arc<dyn SpaceRebuildProgressPort>,
        current_space: Arc<dyn CurrentSpaceIdentityPort>,
    ) -> Self {
        Self {
            progress,
            current_space,
        }
    }

    pub(crate) async fn execute(&self) -> Result<bool, ResetSpaceError> {
        let pending_target = self
            .progress
            .load_target()
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        let current_space_id = self
            .current_space
            .current_space_id()
            .await
            .map_err(|error| ResetSpaceError::FinalizationFailed(error.to_string()))?;
        Ok(pending_target
            .as_ref()
            .is_some_and(|target| current_space_id.as_ref() == Some(target)))
    }
}
