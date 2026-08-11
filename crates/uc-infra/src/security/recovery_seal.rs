//! Restricted recovery channel sealing (ADR-016).
//!
//! Every recovery message is sealed with an application-layer AEAD on top
//! of iroh's authenticated encrypted connection. The one-use handoff key is
//! derived with a dedicated HKDF domain from a mutually verifiable
//! historical transport key (identified by its content-key id), plus both
//! fresh endpoint contributions. The authenticated data binds the protocol
//! version, the space lineage fingerprint, the selected history key number
//! and predecessor generation, both member instances and transport public
//! keys, the change range, the target digest, the request number and the
//! monotonic reply number.
//!
//! The sealed envelope starts with a fixed-length clear header (version,
//! lineage fingerprint, history key number, request/reply numbers, change
//! range, target digest and both member instances) so the receiving side
//! can rebuild the binding before decryption. The current workspace key is
//! never used as a fallback, and no plaintext business material is emitted.

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use uc_core::membership::{
    ContentKeyId, RecoveryBinding, RecoveryChannelMessage, RecoveryEnvelopeHeader,
    RECOVERY_ENVELOPE_HEADER_BYTES,
};

use super::session::InMemorySession;

const RECOVERY_HKDF_INFO: &[u8] = b"uniclipboard-workspace-recovery/v1";
const MAX_RECOVERY_MESSAGE_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RecoverySealError {
    #[error("recovery sealing is unavailable")]
    Unavailable,
    #[error("recovery sealing failed")]
    Failed,
    #[error("recovery message is too large")]
    Oversized,
    #[error("recovery message was rejected")]
    Rejected,
}

/// Seals one recovery message with the one-use handoff key.
///
/// `binding` must carry every handoff fact; the transport public key fields
/// are provided by the transport implementation. `history_key_id`
/// identifies the shared historical transport key in the local session
/// catalog; both endpoints must hold the same key for the same space or the
/// seal cannot be opened. Fresh random nonces are used for every call.
pub fn seal_recovery_message(
    session: &InMemorySession,
    space_id: &uc_core::ids::SpaceId,
    history_key_id: &ContentKeyId,
    binding: &RecoveryBinding,
    message: &RecoveryChannelMessage,
) -> Result<Vec<u8>, RecoverySealError> {
    let resolved = session
        .content_key(
            space_id,
            history_key_id,
            uc_core::membership::ContentKeyPurpose::Transport,
        )
        .map_err(|_| RecoverySealError::Unavailable)?;
    let handoff_key = derive_handoff_key(resolved.key().as_bytes(), binding)?;
    let plaintext = postcard::to_stdvec(message).map_err(|_| RecoverySealError::Failed)?;
    if plaintext.len() > MAX_RECOVERY_MESSAGE_BYTES {
        return Err(RecoverySealError::Oversized);
    }
    let aad = binding.authenticated_data();
    let mut nonce = [0u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let cipher =
        XChaCha20Poly1305::new_from_slice(&handoff_key).map_err(|_| RecoverySealError::Failed)?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| RecoverySealError::Failed)?;
    let header = RecoveryEnvelopeHeader::from_binding(binding);
    let mut envelope = Vec::with_capacity(RECOVERY_ENVELOPE_HEADER_BYTES + 24 + ciphertext.len());
    envelope.extend_from_slice(&header.encode());
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

/// Opens a recovery envelope. Returns the message only when the clear
/// header binds exactly to `binding` and the envelope was sealed with the
/// same historical transport key.
///
/// `binding` must carry the same facts the sealer used; the transport
/// public key fields are provided by the transport implementation from the
/// authenticated connection.
pub fn open_recovery_message(
    session: &InMemorySession,
    space_id: &uc_core::ids::SpaceId,
    history_key_id: &ContentKeyId,
    binding: &RecoveryBinding,
    envelope: &[u8],
) -> Result<RecoveryChannelMessage, RecoverySealError> {
    if envelope.len() < RECOVERY_ENVELOPE_HEADER_BYTES + 24 {
        return Err(RecoverySealError::Rejected);
    }
    let header = RecoveryEnvelopeHeader::decode(&envelope[..RECOVERY_ENVELOPE_HEADER_BYTES])
        .map_err(|_| RecoverySealError::Rejected)?;
    if header.to_binding(
        binding.sender_transport_public_key.clone(),
        binding.receiver_transport_public_key.clone(),
    ) != *binding
    {
        return Err(RecoverySealError::Rejected);
    }
    let resolved = session
        .content_key(
            space_id,
            history_key_id,
            uc_core::membership::ContentKeyPurpose::Transport,
        )
        .map_err(|_| RecoverySealError::Unavailable)?;
    let handoff_key = derive_handoff_key(resolved.key().as_bytes(), binding)?;
    let aad = binding.authenticated_data();
    let cipher =
        XChaCha20Poly1305::new_from_slice(&handoff_key).map_err(|_| RecoverySealError::Failed)?;
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(
                &envelope[RECOVERY_ENVELOPE_HEADER_BYTES..RECOVERY_ENVELOPE_HEADER_BYTES + 24],
            ),
            chacha20poly1305::aead::Payload {
                msg: &envelope[RECOVERY_ENVELOPE_HEADER_BYTES + 24..],
                aad: &aad,
            },
        )
        .map_err(|_| RecoverySealError::Rejected)?;
    postcard::from_bytes(&plaintext).map_err(|_| RecoverySealError::Rejected)
}

/// One-use handoff key: HKDF(salt = both fresh endpoint contributions,
/// ikm = historical transport key) with the dedicated recovery domain.
fn derive_handoff_key(
    history_key: &[u8],
    binding: &RecoveryBinding,
) -> Result<[u8; 32], RecoverySealError> {
    let mut salt = Vec::with_capacity(
        binding.sender_transport_public_key.len() + binding.receiver_transport_public_key.len(),
    );
    salt.extend_from_slice(&binding.sender_transport_public_key);
    salt.extend_from_slice(&binding.receiver_transport_public_key);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), history_key);
    let mut output = [0u8; 32];
    hkdf.expand(RECOVERY_HKDF_INFO, &mut output)
        .map_err(|_| RecoverySealError::Failed)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use uc_core::ids::SpaceId;
    use uc_core::membership::{
        recovery_lineage_fingerprint, AdmissionChangeFacts, ContentKeyId, ContentKeyPurpose,
        RecoveryOffer, RecoveryRequest, WorkspaceChange, WorkspaceChangeKind,
    };

    use super::*;
    use crate::security::{InMemorySession, MasterKey};

    fn session(seed: u8) -> InMemorySession {
        let session = InMemorySession::new();
        session.set_master_key_for_space(
            SpaceId::from_str("space-a"),
            MasterKey::from_bytes(&[seed; 32]).unwrap(),
        );
        session
    }

    fn binding(seed: u8) -> RecoveryBinding {
        RecoveryBinding {
            space_lineage_fingerprint: recovery_lineage_fingerprint("space-a"),
            history_key_number: 1,
            from_epoch: 0,
            sender_instance: [0x01; 32],
            receiver_instance: [0x02; 32],
            sender_transport_public_key: vec![0x11; 32],
            receiver_transport_public_key: vec![0x22; 32],
            from_range_epoch: 0,
            to_range_epoch: 1,
            target_digest: [0x33; 32],
            request_number: seed as u64,
            reply_number: 1,
        }
    }

    fn offer() -> RecoveryOffer {
        RecoveryOffer {
            space_lineage_fingerprint: [0; 32],
            request_number: 7,
            reply_number: 1,
            from_epoch: 0,
            to_epoch: 1,
            has_more: false,
            target_digest: [0x33; 32],
            changes: vec![WorkspaceChange {
                space_lineage: "space-a".to_owned(),
                kind: WorkspaceChangeKind::Admission,
                previous_epoch: 0,
                next_epoch: 1,
                previous_digest: [0; 32],
                digest: [0; 32],
                security_updates: Vec::new(),
                admission: Some(AdmissionChangeFacts {
                    member_instance: uc_core::membership::MemberInstanceId::from_bytes([0x02; 32]),
                    device_id: uc_core::ids::DeviceId::new("device-b"),
                    device_name: "device-b".to_owned(),
                    identity_fingerprint:
                        uc_core::security::IdentityFingerprint::from_display_string(
                            "ABCD-EFGH-IJKL-MNOP",
                        )
                        .unwrap(),
                    transport_public_key: vec![1; 32],
                    transport_address_blob: vec![2; 16],
                    identity_signature: vec![3; 64],
                }),
                removal: None,
                created_at_ms: 1,
            }],
        }
    }

    #[test]
    fn sealed_offer_round_trips_between_members_with_the_same_history_key() {
        let sponsor = session(0x51);
        let joiner = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x07);
        let message = RecoveryChannelMessage::Offer(offer());

        let envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();
        let opened = open_recovery_message(
            &joiner,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &envelope,
        )
        .unwrap();
        assert_eq!(opened, message);
    }

    #[test]
    fn envelope_with_the_wrong_history_key_is_rejected() {
        let sponsor = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x08);
        let message = RecoveryChannelMessage::Offer(offer());
        let envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();

        let attacker = session(0x99);
        let result = open_recovery_message(
            &attacker,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &envelope,
        );
        assert!(matches!(result, Err(RecoverySealError::Rejected)));
    }

    #[test]
    fn tampered_envelope_is_rejected() {
        let sponsor = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x09);
        let message = RecoveryChannelMessage::Offer(offer());
        let mut envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();
        let last = envelope.len() - 1;
        envelope[last] ^= 0x01;
        let result = open_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &envelope,
        );
        assert!(matches!(result, Err(RecoverySealError::Rejected)));
    }

    #[test]
    fn wrong_request_number_in_envelope_is_rejected() {
        let sponsor = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x0a);
        let message = RecoveryChannelMessage::Offer(offer());
        let mut envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();
        envelope[0] ^= 0x01;
        let result = open_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &envelope,
        );
        assert!(matches!(result, Err(RecoverySealError::Rejected)));
    }

    #[test]
    fn tampered_clear_header_is_rejected() {
        let sponsor = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x0e);
        let message = RecoveryChannelMessage::Offer(offer());
        let mut envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();
        envelope[32] ^= 0x01;
        let result = open_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &envelope,
        );
        assert!(matches!(result, Err(RecoverySealError::Rejected)));
    }

    #[test]
    fn cross_space_replay_is_rejected() {
        let sponsor = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x0f);
        let message = RecoveryChannelMessage::Offer(offer());
        let envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();

        let mut wrong_space = binding.clone();
        wrong_space.space_lineage_fingerprint = recovery_lineage_fingerprint("space-b");
        let result = open_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &wrong_space,
            &envelope,
        );
        assert!(matches!(result, Err(RecoverySealError::Rejected)));
    }

    #[test]
    fn wrong_binding_aad_is_rejected() {
        let sponsor = session(0x51);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x0b);
        let message = RecoveryChannelMessage::Offer(offer());
        let envelope = seal_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();

        let mut wrong = binding.clone();
        wrong.target_digest = [0x44; 32];
        let result = open_recovery_message(
            &sponsor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &wrong,
            &envelope,
        );
        assert!(matches!(result, Err(RecoverySealError::Rejected)));
    }

    #[test]
    fn request_seal_also_round_trips() {
        let requester = session(0x61);
        let donor = session(0x61);
        let key_id = ContentKeyId::legacy_v1();
        let binding = binding(0x0c);
        let message = RecoveryChannelMessage::Request(RecoveryRequest {
            space_lineage_fingerprint: [0; 32],
            requester_instance: [0x01; 32],
            requester_device: uc_core::ids::DeviceId::new("device-a"),
            from_epoch: 0,
            from_digest: [0; 32],
            history_key_number: 1,
            request_number: 7,
            request_proof: vec![1; 64],
        });
        let envelope = seal_recovery_message(
            &requester,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &message,
        )
        .unwrap();
        let opened = open_recovery_message(
            &donor,
            &SpaceId::from_str("space-a"),
            &key_id,
            &binding,
            &envelope,
        )
        .unwrap();
        assert_eq!(opened, message);
    }
}
