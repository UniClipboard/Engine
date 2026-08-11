//! Restricted workspace recovery handoff messages (ADR-016).
//!
//! A lagging member that may not yet trust the carrying device can request
//! the continuous change chain over a separate, versioned restricted
//! channel. The channel never carries member names, addresses, member
//! instances, signatures, security material or content in the clear: every
//! message is sealed with an application-layer AEAD bound to the protocol
//! version, space lineage, chosen historical transport key, both member
//! instances and transport public keys, the change range, the target
//! digest, a fresh request number and a monotonic reply number.
//!
//! This module defines the message payloads and their transfer bounds only.
//! Sealing, key derivation and transport live in the infrastructure layer.

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;

use super::workspace_convergence::WorkspaceChange;

/// Stable recovery request/reject category that leaks no member or state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRejection {
    SpaceMismatch,
    RangeOutOfBounds,
    ContinuityMissing,
    IdentityMismatch,
    Unauthorized,
    DigestConflict,
    VersionIncompatible,
}

/// Bounded request from a lagging member to a handoff device.
///
/// Only carries the space lineage, the requester's saved predecessor
/// digest and security generation, the shared historical transport key
/// number, a fresh request number and a request proof verifiable with the
/// historical member material. It cannot enumerate members or probe the
/// current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRequest {
    pub space_lineage_fingerprint: [u8; 32],
    pub requester_instance: [u8; 32],
    pub requester_device: DeviceId,
    /// The requester's saved predecessor security generation.
    pub from_epoch: u64,
    /// The requester's saved predecessor workspace digest.
    pub from_digest: [u8; 32],
    /// Number of the shared historical transport key the requester can prove.
    pub history_key_number: u64,
    /// Fresh request number (reused numbers are rejected).
    pub request_number: u64,
    /// Proof verifiable with the requester's historical member material.
    pub request_proof: Vec<u8>,
}

impl RecoveryRequest {
    pub const MAX_REQUEST_PROOF_BYTES: usize = 4096;

    pub fn validate_transfer_bounds(&self) -> Result<(), RecoveryRejection> {
        if self.request_proof.len() > Self::MAX_REQUEST_PROOF_BYTES {
            return Err(RecoveryRejection::RangeOutOfBounds);
        }
        Ok(())
    }
}

/// One bounded offer batch of the continuous change chain.
///
/// Declares the continuous start/end range, whether another batch remains,
/// and the same target digest; at most [`MAX_OFFER_CHANGES`] changes per
/// offer. The outer envelope carries only version, a random request number
/// and a fixed-length range marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryOffer {
    pub space_lineage_fingerprint: [u8; 32],
    /// Request number this offer answers.
    pub request_number: u64,
    /// Monotonic reply number for this request (replays are rejected).
    pub reply_number: u64,
    /// Continuous range covered by this batch.
    pub from_epoch: u64,
    pub to_epoch: u64,
    /// Whether another batch follows after this one.
    pub has_more: bool,
    pub target_digest: [u8; 32],
    /// The continuous changes of this batch (at most [`MAX_OFFER_CHANGES`]).
    pub changes: Vec<WorkspaceChange>,
}

impl RecoveryOffer {
    pub const MAX_OFFER_CHANGES: usize = 64;

    pub fn validate_transfer_bounds(&self) -> Result<(), RecoveryRejection> {
        if self.changes.len() > Self::MAX_OFFER_CHANGES {
            return Err(RecoveryRejection::RangeOutOfBounds);
        }
        if self.to_epoch < self.from_epoch {
            return Err(RecoveryRejection::RangeOutOfBounds);
        }
        if self.changes.len() != (self.to_epoch - self.from_epoch) as usize {
            return Err(RecoveryRejection::RangeOutOfBounds);
        }
        Ok(())
    }
}

/// Receiver's durable acknowledgement: only confirms the persisted
/// continuous range, the applied digest and the target digest, plus the
/// reply number. Never returns member lists, online state, keys or content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAck {
    pub space_lineage_fingerprint: [u8; 32],
    pub request_number: u64,
    pub reply_number: u64,
    /// Continuous range durably applied by the receiver.
    pub confirmed_epoch: u64,
    /// The applied digest.
    pub applied_digest: [u8; 32],
    /// The target digest this acknowledgement refers to.
    pub target_digest: [u8; 32],
    pub has_more: bool,
}

impl RecoveryAck {
    pub fn validate_transfer_bounds(&self) -> Result<(), RecoveryRejection> {
        Ok(())
    }
}

/// Stable rejection with no member or state disclosure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReject {
    pub space_lineage_fingerprint: [u8; 32],
    pub request_number: u64,
    pub reply_number: u64,
    pub reason: RecoveryRejection,
}

impl RecoveryReject {
    pub fn validate_transfer_bounds(&self) -> Result<(), RecoveryRejection> {
        Ok(())
    }
}

/// The restricted channel's bounded message envelope payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryChannelMessage {
    Request(RecoveryRequest),
    Offer(RecoveryOffer),
    Ack(RecoveryAck),
    Reject(RecoveryReject),
}

impl RecoveryChannelMessage {
    pub fn validate_transfer_bounds(&self) -> Result<(), RecoveryRejection> {
        match self {
            Self::Request(request) => request.validate_transfer_bounds(),
            Self::Offer(offer) => offer.validate_transfer_bounds(),
            Self::Ack(ack) => ack.validate_transfer_bounds(),
            Self::Reject(reject) => reject.validate_transfer_bounds(),
        }
    }
}

/// Versioned identifier of the restricted recovery channel.
pub const WORKSPACE_RECOVERY_CHANNEL_VERSION: &str = "uniclipboard/workspace-recovery/1";

/// Historical transport key numbers start at this value and are reserved
/// for purpose-separated historical keys.
pub const MIN_HISTORY_KEY_NUMBER: u64 = 1;

/// The sealed envelope version written in the clear header.
pub const RECOVERY_ENVELOPE_VERSION: u8 = 1;

/// The binding facts of one recovery handoff. Both endpoints must agree on
/// every field or the seal cannot be opened.
///
/// The transport public key fields are filled by the transport
/// implementation from the authenticated connection; callers provide every
/// other field. The space lineage is carried only as its domain-separated
/// fingerprint, never as the lineage text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryBinding {
    pub space_lineage_fingerprint: [u8; 32],
    /// Number of the shared historical transport key.
    pub history_key_number: u64,
    /// The requester's saved predecessor security generation.
    pub from_epoch: u64,
    /// Sender member instance.
    pub sender_instance: [u8; 32],
    /// Receiver member instance.
    pub receiver_instance: [u8; 32],
    pub sender_transport_public_key: Vec<u8>,
    pub receiver_transport_public_key: Vec<u8>,
    pub from_range_epoch: u64,
    pub to_range_epoch: u64,
    pub target_digest: [u8; 32],
    pub request_number: u64,
    pub reply_number: u64,
}

impl RecoveryBinding {
    /// Deterministic authenticated data for this handoff.
    pub fn authenticated_data(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(256);
        push_field(&mut bytes, WORKSPACE_RECOVERY_CHANNEL_VERSION.as_bytes());
        push_field(&mut bytes, &self.space_lineage_fingerprint);
        push_field(&mut bytes, &self.history_key_number.to_be_bytes());
        push_field(&mut bytes, &self.from_epoch.to_be_bytes());
        push_field(&mut bytes, &self.sender_instance);
        push_field(&mut bytes, &self.receiver_instance);
        push_field(&mut bytes, &self.sender_transport_public_key);
        push_field(&mut bytes, &self.receiver_transport_public_key);
        push_field(&mut bytes, &self.from_range_epoch.to_be_bytes());
        push_field(&mut bytes, &self.to_range_epoch.to_be_bytes());
        push_field(&mut bytes, &self.target_digest);
        push_field(&mut bytes, &self.request_number.to_be_bytes());
        push_field(&mut bytes, &self.reply_number.to_be_bytes());
        bytes
    }
}

fn push_field(buffer: &mut Vec<u8>, value: &[u8]) {
    buffer.extend_from_slice(&(value.len() as u64).to_be_bytes());
    buffer.extend_from_slice(value);
}

/// Fixed-length clear header of one sealed recovery envelope.
///
/// Carries only the version, the lineage fingerprint, the selected history
/// key number, the request/reply numbers, the fixed-length change range,
/// the target digest and both member instances. These fields are required
/// to rebuild the AEAD binding on the receiving side before decryption;
/// they reveal no device material, addresses, signatures, keys or content.
/// The outer iroh connection is itself authenticated and encrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEnvelopeHeader {
    pub space_lineage_fingerprint: [u8; 32],
    pub history_key_number: u64,
    pub request_number: u64,
    pub reply_number: u64,
    pub from_epoch: u64,
    pub to_epoch: u64,
    pub target_digest: [u8; 32],
    pub sender_instance: [u8; 32],
    pub receiver_instance: [u8; 32],
}

/// Size of the fixed-length clear envelope header in bytes.
pub const RECOVERY_ENVELOPE_HEADER_BYTES: usize = 1 + 32 + 8 + 8 + 8 + 8 + 8 + 32 + 32 + 32;

impl RecoveryEnvelopeHeader {
    pub fn from_binding(binding: &RecoveryBinding) -> Self {
        Self {
            space_lineage_fingerprint: binding.space_lineage_fingerprint,
            history_key_number: binding.history_key_number,
            request_number: binding.request_number,
            reply_number: binding.reply_number,
            from_epoch: binding.from_epoch,
            to_epoch: binding.to_range_epoch,
            target_digest: binding.target_digest,
            sender_instance: binding.sender_instance,
            receiver_instance: binding.receiver_instance,
        }
    }

    /// Rebuild a binding from this header and the two authenticated
    /// connection public keys.
    pub fn to_binding(
        &self,
        sender_transport_public_key: Vec<u8>,
        receiver_transport_public_key: Vec<u8>,
    ) -> RecoveryBinding {
        RecoveryBinding {
            space_lineage_fingerprint: self.space_lineage_fingerprint,
            history_key_number: self.history_key_number,
            from_epoch: self.from_epoch,
            sender_instance: self.sender_instance,
            receiver_instance: self.receiver_instance,
            sender_transport_public_key,
            receiver_transport_public_key,
            from_range_epoch: self.from_epoch,
            to_range_epoch: self.to_epoch,
            target_digest: self.target_digest,
            request_number: self.request_number,
            reply_number: self.reply_number,
        }
    }

    pub fn encode(&self) -> [u8; RECOVERY_ENVELOPE_HEADER_BYTES] {
        let mut bytes = [0u8; RECOVERY_ENVELOPE_HEADER_BYTES];
        let mut offset = 0;
        bytes[offset] = RECOVERY_ENVELOPE_VERSION;
        offset += 1;
        bytes[offset..offset + 32].copy_from_slice(&self.space_lineage_fingerprint);
        offset += 32;
        bytes[offset..offset + 8].copy_from_slice(&self.history_key_number.to_be_bytes());
        offset += 8;
        bytes[offset..offset + 8].copy_from_slice(&self.request_number.to_be_bytes());
        offset += 8;
        bytes[offset..offset + 8].copy_from_slice(&self.reply_number.to_be_bytes());
        offset += 8;
        bytes[offset..offset + 8].copy_from_slice(&self.from_epoch.to_be_bytes());
        offset += 8;
        bytes[offset..offset + 8].copy_from_slice(&self.to_epoch.to_be_bytes());
        offset += 8;
        bytes[offset..offset + 32].copy_from_slice(&self.target_digest);
        offset += 32;
        bytes[offset..offset + 32].copy_from_slice(&self.sender_instance);
        offset += 32;
        bytes[offset..offset + 32].copy_from_slice(&self.receiver_instance);
        debug_assert_eq!(offset + 32, RECOVERY_ENVELOPE_HEADER_BYTES);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RecoveryRejection> {
        if bytes.len() != RECOVERY_ENVELOPE_HEADER_BYTES || bytes[0] != RECOVERY_ENVELOPE_VERSION {
            return Err(RecoveryRejection::VersionIncompatible);
        }
        let mut offset = 1;
        let read_u64 = |offset: &mut usize| {
            let value = u64::from_be_bytes(bytes[*offset..*offset + 8].try_into().unwrap());
            *offset += 8;
            value
        };
        let read_array = |offset: &mut usize| {
            let value: [u8; 32] = bytes[*offset..*offset + 32].try_into().unwrap();
            *offset += 32;
            value
        };
        let space_lineage_fingerprint = read_array(&mut offset);
        let history_key_number = read_u64(&mut offset);
        let request_number = read_u64(&mut offset);
        let reply_number = read_u64(&mut offset);
        let from_epoch = read_u64(&mut offset);
        let to_epoch = read_u64(&mut offset);
        let target_digest = read_array(&mut offset);
        let sender_instance = read_array(&mut offset);
        let receiver_instance = read_array(&mut offset);
        Ok(Self {
            space_lineage_fingerprint,
            history_key_number,
            request_number,
            reply_number,
            from_epoch,
            to_epoch,
            target_digest,
            sender_instance,
            receiver_instance,
        })
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RecoveryTransportError {
    #[error("recovery recipient is offline")]
    Offline,
    #[error("recovery request was rejected")]
    Rejected(RecoveryRejection),
    #[error("recovery transport failed")]
    Transport,
}

/// Restricted recovery exchange transport: an unknown carrying device can
/// deliver verifiable continuous material; the source does not gain trust.
#[async_trait::async_trait]
pub trait RecoveryTransportPort: Send + Sync {
    /// Deliver one recovery message to a member device and return the
    /// bounded reply.
    ///
    /// `binding` carries every handoff fact except the two transport public
    /// keys; the implementation fills them from the authenticated connection
    /// and seals the message with the application-layer AEAD before sending.
    async fn exchange_recovery(
        &self,
        recipient: &DeviceId,
        binding: &RecoveryBinding,
        message: RecoveryChannelMessage,
    ) -> Result<RecoveryChannelMessage, RecoveryTransportError>;
}

/// Server side of the restricted recovery channel.
#[async_trait::async_trait]
pub trait RecoveryTransportEndpointPort: Send + Sync {
    /// Handle one inbound recovery message and return the bounded reply.
    /// The implementation verifies the seal, the connection identity, the
    /// request proof and the current-effective-member check before any
    /// offer is released.
    async fn handle_recovery(
        &self,
        source_device: &DeviceId,
        message: RecoveryChannelMessage,
    ) -> Result<RecoveryChannelMessage, RecoveryTransportError>;
}

/// Space lineage fingerprint for recovery binding (never carries the
/// lineage text itself).
pub fn recovery_lineage_fingerprint(lineage: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("uc-workspace-recovery-lineage-v1|{lineage}")).into()
}
