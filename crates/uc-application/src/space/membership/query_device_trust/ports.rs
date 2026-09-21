use async_trait::async_trait;
use uc_core::ids::DeviceId;

use super::{
    AdmissionDisplayStatus, DeviceTrustObservation, PairingConfirmationTarget,
    QueryDeviceTrustError, SpaceDeviceUpdateStatus,
};
use crate::space::admission::CurrentJoinStatus;

#[async_trait]
pub trait LoadDeviceTrustObservationsPort: Send + Sync {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError>;
}

#[async_trait]
pub(crate) trait LoadSecurityDeviceUpdateStatusPort: Send + Sync {
    async fn load_security_device_update_status(
        &self,
    ) -> Result<SpaceDeviceUpdateStatus, QueryDeviceTrustError>;
}

#[async_trait]
pub trait LoadCurrentJoinStatusPort: Send + Sync {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError>;

    async fn load_admission_display(
        &self,
        _targets: &[PairingConfirmationTarget],
    ) -> Result<AdmissionDisplayStatus, QueryDeviceTrustError> {
        Ok(AdmissionDisplayStatus {
            current_join: self.load_current_join().await?,
            inbound_pairings: Vec::new(),
            pending_inbound_member: None,
            pairing_confirmations: Vec::new(),
        })
    }
}
