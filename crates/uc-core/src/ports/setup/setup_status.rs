use crate::{ids::SpaceId, setup::SetupStatus};
use async_trait::async_trait;

#[async_trait]
pub trait SetupStatusPort: Send + Sync {
    async fn get_status(&self) -> anyhow::Result<SetupStatus>;
    async fn set_status(&self, status: &SetupStatus) -> anyhow::Result<()>;

    async fn get_legacy_isolation_target(&self) -> anyhow::Result<Option<SpaceId>> {
        Ok(None)
    }

    async fn set_legacy_isolation_target(&self, _space_id: &SpaceId) -> anyhow::Result<()> {
        anyhow::bail!("legacy isolation progress persistence is unavailable")
    }

    async fn clear_legacy_isolation_target(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
