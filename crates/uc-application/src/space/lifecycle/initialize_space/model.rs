use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::security::IdentityFingerprint;

#[derive(Debug)]
pub(crate) struct InitializeSpaceRequest {
    pub(crate) passphrase: Passphrase,
    pub(crate) passphrase_confirm: Passphrase,
    pub(crate) device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializeSpaceResult {
    pub space_id: SpaceId,
    pub self_device_id: DeviceId,
    pub fingerprint: IdentityFingerprint,
}
