use async_trait::async_trait;
use uc_core::ids::DeviceId;

use super::{DeviceTrustObservation, QueryDeviceTrustError};

#[async_trait]
pub trait LoadDeviceTrustObservationsPort: Send + Sync {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError>;
}
