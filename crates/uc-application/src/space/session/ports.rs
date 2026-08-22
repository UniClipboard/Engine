use async_trait::async_trait;
use uc_core::crypto::domain::ActiveSpace;
use uc_core::ids::SpaceId;
use uc_core::ports::space::SpaceAccessError;

#[async_trait]
pub trait IsSpaceUnlockedPort: Send + Sync {
    async fn is_unlocked(&self, space_id: &SpaceId) -> bool;
}

#[async_trait]
pub trait ResumeSpaceSessionPort: Send + Sync {
    async fn try_resume_session(
        &self,
        space_id: &SpaceId,
    ) -> Result<Option<ActiveSpace>, SpaceAccessError>;
}
