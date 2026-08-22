use std::sync::Arc;

use crate::space::current_space::CurrentSpaceIdentityPort;
use crate::space::session::SpaceSessionActivity;

use super::{LockSpacePort, LockSpaceSessionError};

pub struct LockSpaceSessionUseCase {
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    lock: Arc<dyn LockSpacePort>,
    activity: Arc<SpaceSessionActivity>,
}

impl LockSpaceSessionUseCase {
    pub fn new(
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        lock: Arc<dyn LockSpacePort>,
        activity: Arc<SpaceSessionActivity>,
    ) -> Self {
        Self {
            current_space_identity,
            lock,
            activity,
        }
    }

    pub async fn execute(&self) -> Result<(), LockSpaceSessionError> {
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
