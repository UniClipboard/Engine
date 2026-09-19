use std::sync::Arc;

use crate::space::lifecycle::CurrentSpaceIdentityPort;
use crate::space::lifecycle::SpaceSessionRecoveryPort;

use super::{LockSpacePort, LockSpaceSessionError};

pub(crate) struct LockSpaceSessionUseCase {
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    lock: Arc<dyn LockSpacePort>,
    recovery: Arc<dyn SpaceSessionRecoveryPort>,
}

impl LockSpaceSessionUseCase {
    pub(crate) fn new(
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        lock: Arc<dyn LockSpacePort>,
        recovery: Arc<dyn SpaceSessionRecoveryPort>,
    ) -> Self {
        Self {
            current_space_identity,
            lock,
            recovery,
        }
    }

    pub(crate) async fn execute(&self) -> Result<(), LockSpaceSessionError> {
        let space_id = self
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|error| LockSpaceSessionError::CurrentSpace(error.to_string()))?
            .ok_or(LockSpaceSessionError::NotInitialized)?;

        let lock_generation = self.recovery.pause_for_lock().await?;
        if self.lock.lock(&space_id).await.is_ok() {
            self.recovery.finish_successful_lock(lock_generation).await;
            return Ok(());
        }
        self.recovery
            .restore_after_failed_lock(lock_generation)
            .await
            .map_err(LockSpaceSessionError::RecoveryFailed)?;
        Err(LockSpaceSessionError::LockFailed)
    }
}
