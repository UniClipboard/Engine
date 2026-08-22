use std::sync::Arc;

use super::error::ResetSpaceError;
use super::ports::PendingSpaceInvitationResetPort;
use crate::space::rebuild_space::RebuildSpaceUseCase;

pub(crate) struct ResetSpaceUseCase {
    rebuild_space: Arc<RebuildSpaceUseCase>,
    pending_invitations: Arc<dyn PendingSpaceInvitationResetPort>,
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
