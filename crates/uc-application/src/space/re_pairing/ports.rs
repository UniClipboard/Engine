use async_trait::async_trait;

use super::RePairingStateError;

#[async_trait]
pub trait RePairingStateStorePort: Send + Sync {
    async fn is_required(&self) -> Result<bool, RePairingStateError>;

    async fn set_required(&self, required: bool) -> Result<(), RePairingStateError>;
}
