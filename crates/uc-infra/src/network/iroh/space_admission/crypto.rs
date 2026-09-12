use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256, Sha512};
use uc_application::deps::SpaceAdmissionTransportError;
use uc_core::membership::{
    AdmissionChannelPeerId, AdmissionContinuationCredential, SpaceAdmissionId,
};

use super::super::trace_context::WireTraceContext;

type HmacSha512 = Hmac<Sha512>;

/// 证明计算只报告密码操作结果，由协议拥有者解释失败。
#[derive(Debug, thiserror::Error)]
pub(super) enum ProofError {
    #[error("admission proof has invalid length")]
    InvalidLength,
    #[error("admission proof key is invalid")]
    InvalidKey(#[source] hmac::digest::InvalidLength),
    #[error("admission proof did not verify")]
    Rejected(#[source] hmac::digest::MacError),
}

pub(super) fn peer_id(
    bytes: &[u8; 32],
) -> Result<AdmissionChannelPeerId, SpaceAdmissionTransportError> {
    AdmissionChannelPeerId::from_bytes(*bytes)
        .ok_or(SpaceAdmissionTransportError::AuthenticationRejected)
}

pub(super) fn copy_credential(
    credential: &AdmissionContinuationCredential,
) -> Result<AdmissionContinuationCredential, SpaceAdmissionTransportError> {
    AdmissionContinuationCredential::from_bytes(credential.as_bytes().to_vec())
        .map_err(|_| SpaceAdmissionTransportError::Unavailable)
}

pub(super) fn random_nonce() -> [u8; 32] {
    let mut nonce = [0u8; 32];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

pub(super) fn calculate_mac(
    credential: &AdmissionContinuationCredential,
    direction: &[u8],
    admission_id: SpaceAdmissionId,
    sender: AdmissionChannelPeerId,
    receiver: AdmissionChannelPeerId,
    nonce: &[u8; 32],
    digest: &[u8; 32],
    trace_context: Option<&WireTraceContext>,
) -> Result<Vec<u8>, ProofError> {
    let mut mac =
        HmacSha512::new_from_slice(credential.as_bytes()).map_err(ProofError::InvalidKey)?;
    update_mac(
        &mut mac,
        direction,
        admission_id,
        sender,
        receiver,
        nonce,
        digest,
        trace_context,
    );
    Ok(mac.finalize().into_bytes().to_vec())
}

pub(super) fn verify_mac(
    credential: &AdmissionContinuationCredential,
    direction: &[u8],
    admission_id: SpaceAdmissionId,
    sender: AdmissionChannelPeerId,
    receiver: AdmissionChannelPeerId,
    nonce: &[u8; 32],
    digest: &[u8; 32],
    trace_context: Option<&WireTraceContext>,
    provided: &[u8],
) -> Result<(), ProofError> {
    if provided.len() != 64 {
        return Err(ProofError::InvalidLength);
    }
    let mut mac =
        HmacSha512::new_from_slice(credential.as_bytes()).map_err(ProofError::InvalidKey)?;
    update_mac(
        &mut mac,
        direction,
        admission_id,
        sender,
        receiver,
        nonce,
        digest,
        trace_context,
    );
    mac.verify_slice(provided).map_err(ProofError::Rejected)
}

fn update_mac(
    mac: &mut HmacSha512,
    direction: &[u8],
    admission_id: SpaceAdmissionId,
    sender: AdmissionChannelPeerId,
    receiver: AdmissionChannelPeerId,
    nonce: &[u8; 32],
    digest: &[u8; 32],
    trace_context: Option<&WireTraceContext>,
) {
    mac.update(b"uc-space-admission-channel-v1");
    mac.update(direction);
    mac.update(admission_id.as_bytes());
    mac.update(sender.as_bytes());
    mac.update(receiver.as_bytes());
    mac.update(nonce);
    mac.update(digest);
    mac.update(&trace_context_digest(trace_context));
}

fn trace_context_digest(trace_context: Option<&WireTraceContext>) -> [u8; 32] {
    let encoded = trace_context
        .and_then(|context| postcard::to_stdvec(context).ok())
        .unwrap_or_default();
    Sha256::digest(encoded).into()
}
