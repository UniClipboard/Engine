use std::sync::Arc;

use tracing::{debug, info, instrument, warn};
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::SpaceId;
use uc_core::ports::space::SpaceAccessError;
use uc_observability_contract::analytics::{AnalyticsFacade, Event, UnlockFailureReason};

use crate::space::current_space::CurrentSpaceIdentityPort;

use super::error::UnlockSpaceError;
use super::ports::UnlockSpacePort;
use super::readiness::PostSessionReadiness;

pub(crate) struct UnlockSpaceUseCase {
    space_access: Arc<dyn UnlockSpacePort>,
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    readiness: Arc<PostSessionReadiness>,
    analytics: Arc<dyn AnalyticsFacade>,
}

impl UnlockSpaceUseCase {
    pub(crate) fn new(
        space_access: Arc<dyn UnlockSpacePort>,
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        readiness: Arc<PostSessionReadiness>,
        analytics: Arc<dyn AnalyticsFacade>,
    ) -> Self {
        Self {
            space_access,
            current_space_identity,
            readiness,
            analytics,
        }
    }

    #[instrument(skip_all)]
    pub(crate) async fn execute(
        &self,
        passphrase: Passphrase,
    ) -> Result<SpaceId, UnlockSpaceError> {
        let space_id = self.unlock(passphrase).await?;

        self.readiness
            .complete_after_unlock()
            .await
            .map_err(UnlockSpaceError::Internal)?;

        Ok(space_id)
    }

    async fn unlock(&self, passphrase: Passphrase) -> Result<SpaceId, UnlockSpaceError> {
        let space_id = match self.current_space_identity.current_space_id().await {
            Ok(Some(space_id)) => space_id,
            Ok(None) => {
                debug!("unlock rejected: current Space is absent");
                return Err(UnlockSpaceError::SetupNotCompleted);
            }
            Err(error) => {
                self.analytics.capture(Event::SpaceUnlockFailed {
                    failure_reason: UnlockFailureReason::Internal,
                });
                return Err(UnlockSpaceError::Internal(error.to_string()));
            }
        };

        match self.space_access.unlock(&space_id, &passphrase).await {
            Ok(_) => {
                info!("space unlocked");
                self.analytics.capture(Event::SpaceUnlocked);
                Ok(space_id)
            }
            Err(error) => {
                self.analytics.capture(Event::SpaceUnlockFailed {
                    failure_reason: unlock_failure_reason(&error),
                });
                Err(map_unlock_error(error))
            }
        }
    }
}

fn unlock_failure_reason(error: &SpaceAccessError) -> UnlockFailureReason {
    match error {
        SpaceAccessError::WrongPassphrase => UnlockFailureReason::PassphraseMismatch,
        SpaceAccessError::NotInitialized => UnlockFailureReason::SpaceNotFound,
        SpaceAccessError::CorruptedKeyMaterial => UnlockFailureReason::KeyslotCorrupted,
        _ => UnlockFailureReason::Internal,
    }
}

fn map_unlock_error(error: SpaceAccessError) -> UnlockSpaceError {
    match error {
        SpaceAccessError::NotInitialized => UnlockSpaceError::SpaceNotInitialized,
        SpaceAccessError::WrongPassphrase => UnlockSpaceError::WrongPassphrase,
        SpaceAccessError::CorruptedKeyMaterial => UnlockSpaceError::CorruptedKeyMaterial,
        SpaceAccessError::Internal(message) => UnlockSpaceError::Internal(message),
        other => {
            warn!(error = %other, "unexpected space access error during unlock");
            UnlockSpaceError::Internal(other.to_string())
        }
    }
}
