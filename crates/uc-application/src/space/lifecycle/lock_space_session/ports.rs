use async_trait::async_trait;
use uc_core::ids::SpaceId;
use uc_core::ports::space::SpaceAccessError;

#[async_trait]
pub trait LockSpacePort: Send + Sync {
    async fn lock(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError>;
}
