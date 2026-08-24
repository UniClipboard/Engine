//! Shared values used by the joiner and sponsor admission seams (ADR-017).
//!
//! The role-specific interfaces live with `joiner` and `sponsor`; this module
//! only retains values and request bindings shared across those roles.

use uc_core::ids::DeviceId;
use uc_core::space_access::PreparedGroupJoin;

pub(crate) fn stable_join_request_binding(
    device_id: &DeviceId,
    identity_fingerprint: &uc_core::security::IdentityFingerprint,
) -> Vec<u8> {
    let mut binding = b"uniclipboard/join-request-binding/v1\0".to_vec();
    let device = device_id.as_str().as_bytes();
    binding.extend_from_slice(&(device.len() as u64).to_be_bytes());
    binding.extend_from_slice(device);
    let fingerprint = identity_fingerprint.as_display().as_bytes();
    binding.extend_from_slice(&(fingerprint.len() as u64).to_be_bytes());
    binding.extend_from_slice(fingerprint);
    binding
}

#[derive(Debug)]
pub(crate) struct DurableLocalJoinPreparation {
    pub attempt_id: [u8; 32],
    pub join_id: [u8; 16],
    pub request_message_id: [u8; 32],
    pub resume_public_key: Vec<u8>,
    pub prepared_group_join: PreparedGroupJoin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DurableJoinerCompletion {
    Active(uc_core::pairing::DurableAdmissionFrame),
    SpaceTransitionRequired,
}
