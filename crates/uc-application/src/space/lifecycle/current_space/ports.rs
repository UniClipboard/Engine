use async_trait::async_trait;
use uc_core::ids::SpaceId;

use super::CurrentSpaceIdentityError;

#[async_trait]
pub trait CurrentSpaceIdentityPort: Send + Sync {
    async fn current_space_id(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError>;

    async fn requires_legacy_profile_isolation(&self) -> Result<bool, CurrentSpaceIdentityError> {
        Ok(self.current_space_id().await?.is_some())
    }
}

#[async_trait]
pub trait InitialSpaceActivationPort: Send + Sync {
    async fn activate_initial_space(
        &self,
        space_id: &SpaceId,
    ) -> Result<(), CurrentSpaceIdentityError>;
}

#[async_trait]
pub trait PortableCurrentSpaceIdentityPort: Send + Sync {
    async fn prepare_portable_identity(&self) -> Result<(), CurrentSpaceIdentityError>;
}
