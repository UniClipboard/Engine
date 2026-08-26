use std::sync::Arc;

use uc_core::ports::space::SpaceAccessError;

use crate::space::lifecycle::CurrentSpaceIdentityPort;
use crate::space::lifecycle::PostSessionReadiness;
use crate::space::lifecycle::{ResumeSpaceSessionPort, SpaceSessionActivityPort};

use super::{RecoverSpaceSessionError, RecoverSpaceSessionResult};

pub(crate) struct RecoverSpaceSessionUseCase {
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    resume_session: Arc<dyn ResumeSpaceSessionPort>,
    readiness: Arc<PostSessionReadiness>,
    activity: Arc<dyn SpaceSessionActivityPort>,
}

impl RecoverSpaceSessionUseCase {
    pub(crate) fn new(
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        resume_session: Arc<dyn ResumeSpaceSessionPort>,
        readiness: Arc<PostSessionReadiness>,
        activity: Arc<dyn SpaceSessionActivityPort>,
    ) -> Self {
        Self {
            current_space_identity,
            resume_session,
            readiness,
            activity,
        }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<RecoverSpaceSessionResult, RecoverSpaceSessionError> {
        let Some(space_id) = self
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|error| RecoverSpaceSessionError::CurrentSpace(error.to_string()))?
        else {
            return Ok(not_recovered());
        };

        let resumed = match self.resume_session.try_resume_session(&space_id).await {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(SpaceAccessError::CorruptedKeyMaterial) => {
                return Err(RecoverSpaceSessionError::CorruptedKeyMaterial);
            }
            Err(SpaceAccessError::NotInitialized) | Err(SpaceAccessError::WrongPassphrase) => {
                return Err(RecoverSpaceSessionError::KeyringMiss);
            }
            Err(error) => return Err(RecoverSpaceSessionError::Internal(error.to_string())),
        };
        if !resumed {
            return Ok(not_recovered());
        }

        self.readiness
            .complete_after_resume()
            .await
            .map_err(RecoverSpaceSessionError::Internal)?;
        self.activity.resume_after_session_ready().await?;

        Ok(RecoverSpaceSessionResult {
            unlocked: true,
            resumed: true,
        })
    }
}

fn not_recovered() -> RecoverSpaceSessionResult {
    RecoverSpaceSessionResult {
        unlocked: false,
        resumed: false,
    }
}
