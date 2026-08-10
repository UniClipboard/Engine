//! Mobile-sync dependency groupings (ADR-018 stage 4).
//!
//! These port bundles belong to the LAN compatibility line; `uc-application`
//! no longer carries them so the default P2P dependency closure stays free
//! of LAN-only port types.

use std::sync::Arc;

use uc_core::ports::mobile_sync::{
    DeleteMobileDevicePort, FindMobileDeviceByIdPort, FindMobileDeviceByUsernamePort,
    ListMobileDevicesPort, MobileSyncEndpointInfoPort, SaveMobileDevicePort,
    UpdateMobileDevicePort,
};

#[derive(Clone)]
pub struct MobileDevicePorts {
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
