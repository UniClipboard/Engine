//! Mobile-sync dependency groupings (ADR-018 stage 4).
//!
//! These port bundles belong to the LAN compatibility line; `uc-application`
//! no longer carries them so the default P2P dependency closure stays free
//! of LAN-only port types.

use async_trait::async_trait;
use std::sync::Arc;
use uc_core::mobile_sync::MobileDeviceId;
use uc_core::ports::mobile_sync::{
    DeleteMobileDevicePort, FindMobileDeviceByIdPort, FindMobileDeviceByUsernamePort,
    ListMobileDevicesPort, MobileActivityError, MobileDeviceStore, MobileSyncEndpointInfoPort,
    SaveMobileDevicePort, UpdateMobileDevicePort,
};

/// 请求流程只依赖活动写入能力，不接触设备资料更新。
#[async_trait]
pub trait RecordMobileDeviceActivityPort: Send + Sync {
    async fn record_activity(
        &self,
        device_id: &MobileDeviceId,
        observed_at_ms: i64,
    ) -> Result<bool, MobileActivityError>;
}

#[async_trait]
impl<T: MobileDeviceStore> RecordMobileDeviceActivityPort for T {
    async fn record_activity(
        &self,
        device_id: &MobileDeviceId,
        observed_at_ms: i64,
    ) -> Result<bool, MobileActivityError> {
        MobileDeviceStore::record_activity(self, device_id, observed_at_ms).await
    }
}

#[derive(Clone)]
pub struct MobileDevicePorts {
    pub activity: Arc<dyn RecordMobileDeviceActivityPort>,
    pub find_by_username: Arc<dyn FindMobileDeviceByUsernamePort>,
    pub find_by_id: Arc<dyn FindMobileDeviceByIdPort>,
    pub list: Arc<dyn ListMobileDevicesPort>,
    pub save: Arc<dyn SaveMobileDevicePort>,
    pub delete: Arc<dyn DeleteMobileDevicePort>,
    pub update: Arc<dyn UpdateMobileDevicePort>,
}

#[derive(Clone)]
pub struct MobileSyncPorts {
    pub devices: MobileDevicePorts,
    pub endpoint_info: Arc<dyn MobileSyncEndpointInfoPort>,
}
