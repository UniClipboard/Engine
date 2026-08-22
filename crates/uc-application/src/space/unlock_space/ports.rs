use async_trait::async_trait;
use uc_core::crypto::domain::{ActiveSpace, Passphrase};
use uc_core::ids::SpaceId;
use uc_core::ports::space::SpaceAccessError;

#[async_trait]
pub trait UnlockSpacePort: Send + Sync {
    async fn unlock(
        &self,
        space_id: &SpaceId,
        passphrase: &Passphrase,
    ) -> Result<ActiveSpace, SpaceAccessError>;
}
