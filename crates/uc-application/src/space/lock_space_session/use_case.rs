use std::sync::Arc;

use crate::space::current_space::CurrentSpaceIdentityPort;
use crate::space::session::SpaceSessionActivityPort;

use super::{LockSpacePort, LockSpaceSessionError};

pub(crate) struct LockSpaceSessionUseCase {
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    lock: Arc<dyn LockSpacePort>,
    activity: Arc<dyn SpaceSessionActivityPort>,
}

impl LockSpaceSessionUseCase {
    pub(crate) fn new(
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        lock: Arc<dyn LockSpacePort>,
        activity: Arc<dyn SpaceSessionActivityPort>,
    ) -> Self {
        Self {
            current_space_identity,
            lock,
            activity,
        }
    }

    pub(crate) async fn execute(&self) -> Result<(), LockSpaceSessionError> {
        let space_id = self
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|error| LockSpaceSessionError::CurrentSpace(error.to_string()))?
            .ok_or(LockSpaceSessionError::NotInitialized)?;

        self.activity.pause_for_lock().await?;
        if self.lock.lock(&space_id).await.is_ok() {
            return Ok(());
        }
        self.activity
            .restore_after_failed_lock()
            .await
            .map_err(LockSpaceSessionError::RecoveryFailed)?;
        Err(LockSpaceSessionError::LockFailed)
    }
}
