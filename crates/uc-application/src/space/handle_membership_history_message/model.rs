use uc_core::ids::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedMember {
    device_id: DeviceId,
}

impl AuthenticatedMember {
    pub fn new(device_id: DeviceId) -> Self {
        Self { device_id }
    }

    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }
}
