use async_trait::async_trait;
use uc_core::ids::DeviceId;

use super::{DeviceTrustObservation, QueryDeviceTrustError};
use crate::space::admission::CurrentJoinStatus;

#[async_trait]
pub trait LoadDeviceTrustObservationsPort: Send + Sync {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError>;
}

#[async_trait]
pub trait LoadCurrentJoinStatusPort: Send + Sync {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError>;
}
