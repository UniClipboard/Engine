pub struct PendingJoinerCompleteAck {
    pub sponsor_device_id: uc_core::DeviceId,
    pub frame: uc_core::pairing::DurableAdmissionFrame,
}
