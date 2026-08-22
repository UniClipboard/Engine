use async_trait::async_trait;
use uc_core::ids::SpaceId;

use super::CurrentSpaceIdentityError;

#[async_trait]
pub trait CurrentSpaceIdentityPort: Send + Sync {
    async fn current_space_id(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError>;
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
