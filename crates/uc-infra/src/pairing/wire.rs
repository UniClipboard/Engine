//! Binary codec for [`PairingSessionMessage`].
//!
//! Slice 1 pairing session wire format (postcard + explicit version byte).
//! Runs over an iroh bi-directional stream; P7c.2 layers length-prefixed
//! framing on top of this codec before hitting the stream.
//!
//! Design notes:
//!
//! * **Wire types are infra-local.** The core [`PairingSessionMessage`]
//!   carries no `serde` derives (§6.3). This module owns mirror structs with
//!   serde derives and maps them at the boundary.
//! * **Envelope carries a version byte from day 1.** Slice 2+ will extend
//!   the enum (e.g. keep-alives, resume tokens); `v` lets us distinguish
//!   "old peer sent unknown variant" from "data corruption".
//! * **postcard, not JSON.** postcard gives ~40% smaller payloads than
//!   JSON for this shape (mainly because keyslot / challenge / nonce are
//!   binary bytes). Rendezvous tickets are already ~500 bytes — saving here
//!   is worth the binary opaqueness.
//! * **IdentityFingerprint on the wire uses the display form**
//!   (`ABCD-EFGH-IJKL-MNOP`) — stable, printable in logs, round-trips
//!   through [`IdentityFingerprint::from_display_string`].
//!
//! [`PairingSessionMessage`]: uc_core::pairing::PairingSessionMessage

use serde::{Deserialize, Serialize};
use thiserror::Error;

use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{AdmissionChangeFacts, MemberInstanceId};
use uc_core::pairing::{
    DurableAdmissionFrame, DurableAdmissionMessageKind, InvitationCode, JoinerChallengeResponse,
    JoinerRequest, PairingReject, PairingRejectReason, PairingSecurityCapability,
    PairingSessionMessage, SponsorAdmissionOffer,
};
use uc_core::ports::pairing::PairingSessionId;
use uc_core::security::IdentityFingerprint;

pub(crate) const MAX_FRAME_SIZE: usize = 4 * 1024 * 1024;

/// Wire 版本号。
///
/// 升版历史：
/// postcard 非 schema-兼容，每次新增字段都升版本号；旧 peer 发来的低版本帧会走
/// [`WireDecodeError::UnsupportedVersion`] 分支显式拒连，让排障信号明确。
const WIRE_VERSION: u8 = 10;

// ============================================================================
// Wire types (infra-local)
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
enum WireBody {
    Request(WireJoinerRequest),
    AdmissionOffer(WireSponsorAdmissionOffer),
    ChallengeResponse(WireJoinerChallengeResponse),
    DurableAdmission(WireDurableAdmissionFrame),
    Reject(WirePairingReject),
}

#[derive(Serialize, Deserialize, Debug)]
struct WireJoinerRequest {
    attempt_id: [u8; 32],
    join_id: [u8; 16],
    request_message_id: [u8; 32],
    invitation_code: String,
    device_id: String,
    device_name: String,
    identity_fingerprint: String,
    nonce: Vec<u8>,
    /// Slice 2 Phase 1 · T5：joiner 传输地址不透明字节（iroh postcard
    /// 编码的 `EndpointAddr`）。
    ///
    /// postcard 按结构体字段顺序追加，新增字段不是 schema-兼容的——
    /// Slice 1→Slice 2 升级期的跨版本对端不兼容通过 [`WIRE_VERSION`]
    /// 升到 `2` 来显式拒连，由 [`WireDecodeError::UnsupportedVersion`]
    /// 提示用户升级；生产前未发布，不需要兼容层。
    ///
    /// 空 `Vec` 是一个合法业务值：表示本端 adapter 暂时没有可发布的
    /// direct addr（例如 endpoint 还未 online），sponsor 端收到后跳过
    /// `peer_addr_repo.upsert`，presence 下次 `ensure_reachable_all`
    /// 从 rendezvous 再拉兜底。
    transport_address_blob: Vec<u8>,
    security_capability: u8,
    key_package: Vec<u8>,
    member_instance: [u8; 32],
    membership_credential: WireMembershipCredential,
    resume_public_key: Vec<u8>,
    admission: WireAdmissionFacts,
}

#[derive(Serialize, Deserialize, Debug)]
struct WireMembershipCredential {
    credential_format_version: u16,
    signature_algorithm_version: u16,
    public_key: Vec<u8>,
    credential_id: [u8; 32],
}

#[derive(Serialize, Deserialize, Debug)]
struct WireDurableAdmissionFrame {
    attempt_id: [u8; 32],
    kind: u8,
    message_id: [u8; 32],
    predecessor_message_id: Option<[u8; 32]>,
    payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
struct WireSponsorAdmissionOffer {
    space_id: String,
    kdf_parameters_blob: Vec<u8>,
    challenge: Vec<u8>,
    pairing_session_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct WireJoinerChallengeResponse {
    encrypted_challenge: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
struct WireAdmissionFacts {
    member_instance: [u8; 32],
    device_id: String,
    device_name: String,
    identity_fingerprint: String,
    transport_public_key: Vec<u8>,
    transport_address_blob: Vec<u8>,
    identity_signature: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
struct WirePairingReject {
    reason: WireRejectReason,
}

#[derive(Serialize, Deserialize, Debug)]
enum WireRejectReason {
    InvitationMismatch,
    AdmissionUnavailable,
    AdmissionConflict,
    PassphraseMismatch,
    UserRejected,
    Timeout,
    Internal(String),
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum WireEncodeError {
    #[error("postcard encode failed: {0}")]
    Postcard(#[from] postcard::Error),

    #[error("pairing frame length {len} exceeds maximum {max}")]
    FrameTooLarge { len: usize, max: usize },
}

#[derive(Debug, Error)]
pub enum WireDecodeError {
    #[error("postcard decode failed: {0}")]
    Postcard(postcard::Error),

    #[error("unsupported wire version {got} (this build understands {expected})")]
    UnsupportedVersion { got: u8, expected: u8 },

    #[error("invalid identity fingerprint on wire: {0}")]
    InvalidFingerprint(String),

    #[error("unsupported pairing security capability {0}")]
    UnsupportedSecurityCapability(u8),

    #[error("unsupported durable admission message kind {0}")]
    UnsupportedDurableAdmissionKind(u8),

    #[error("invalid durable join request: {0}")]
    InvalidDurableJoinRequest(String),

    #[error("pairing frame length {len} exceeds maximum {max}")]
    FrameTooLarge { len: usize, max: usize },
}

impl From<postcard::Error> for WireDecodeError {
    fn from(err: postcard::Error) -> Self {
        WireDecodeError::Postcard(err)
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Serialize a [`PairingSessionMessage`] for transport.
pub fn encode(message: &PairingSessionMessage) -> Result<Vec<u8>, WireEncodeError> {
    let mut encoded = vec![WIRE_VERSION];
    encoded.extend(postcard::to_allocvec(&to_wire(message))?);
    if encoded.len() > MAX_FRAME_SIZE {
        return Err(WireEncodeError::FrameTooLarge {
            len: encoded.len(),
            max: MAX_FRAME_SIZE,
        });
    }
    Ok(encoded)
}

/// Deserialize a [`PairingSessionMessage`] from bytes produced by
/// [`encode`] (or a peer running a compatible version).
pub fn decode(bytes: &[u8]) -> Result<PairingSessionMessage, WireDecodeError> {
    if bytes.len() > MAX_FRAME_SIZE {
        return Err(WireDecodeError::FrameTooLarge {
            len: bytes.len(),
            max: MAX_FRAME_SIZE,
        });
    }
    let Some((&version, body)) = bytes.split_first() else {
        let error =
            postcard::from_bytes::<WireBody>(&[]).expect_err("empty postcard body must fail");
        return Err(WireDecodeError::Postcard(error));
    };
    if version != WIRE_VERSION {
        return Err(WireDecodeError::UnsupportedVersion {
            got: version,
            expected: WIRE_VERSION,
        });
    }
    from_wire(postcard::from_bytes(body)?)
}

// ============================================================================
// Conversions
// ============================================================================

fn to_wire(msg: &PairingSessionMessage) -> WireBody {
    match msg {
        PairingSessionMessage::Request(r) => WireBody::Request(WireJoinerRequest {
            attempt_id: r.attempt_id,
            join_id: r.join_id,
            request_message_id: r.request_message_id,
            invitation_code: r.invitation_code.as_str().to_string(),
            device_id: r.device_id.as_str().to_string(),
            device_name: r.device_name.clone(),
            identity_fingerprint: r.identity_fingerprint.as_display().to_string(),
            nonce: r.nonce.clone(),
            transport_address_blob: r.transport_address_blob.clone(),
            security_capability: match r.security_capability {
                PairingSecurityCapability::ReliableGroupEpochV1 => 1,
            },
            key_package: r.key_package.clone(),
            member_instance: *r.member_instance.as_bytes(),
            membership_credential: WireMembershipCredential {
                credential_format_version: r.membership_credential.credential_format_version,
                signature_algorithm_version: r.membership_credential.signature_algorithm_version,
                public_key: r.membership_credential.public_key.clone(),
                credential_id: *r.membership_credential.credential_id.as_bytes(),
            },
            resume_public_key: r.resume_public_key.clone(),
            admission: WireAdmissionFacts {
                member_instance: *r.admission.member_instance.as_bytes(),
                device_id: r.admission.device_id.as_str().to_string(),
                device_name: r.admission.device_name.clone(),
                identity_fingerprint: r.admission.identity_fingerprint.as_display().to_string(),
                transport_public_key: r.admission.transport_public_key.clone(),
                transport_address_blob: r.admission.transport_address_blob.clone(),
                identity_signature: r.admission.identity_signature.clone(),
            },
        }),
        PairingSessionMessage::AdmissionOffer(o) => {
            WireBody::AdmissionOffer(WireSponsorAdmissionOffer {
                space_id: o.space_id.inner().clone(),
                kdf_parameters_blob: o.kdf_parameters_blob.clone(),
                challenge: o.challenge.clone(),
                pairing_session_id: o.pairing_session_id.as_str().to_string(),
            })
        }
        PairingSessionMessage::ChallengeResponse(c) => {
            WireBody::ChallengeResponse(WireJoinerChallengeResponse {
                encrypted_challenge: c.encrypted_challenge.clone(),
            })
        }
        PairingSessionMessage::DurableAdmission(frame) => {
            WireBody::DurableAdmission(WireDurableAdmissionFrame {
                attempt_id: frame.attempt_id,
                kind: match frame.kind {
                    DurableAdmissionMessageKind::Candidate => 1,
                    DurableAdmissionMessageKind::Prepared => 2,
                    DurableAdmissionMessageKind::Commit => 3,
                    DurableAdmissionMessageKind::Applied => 4,
                    DurableAdmissionMessageKind::Complete => 5,
                    DurableAdmissionMessageKind::CompleteAck => 6,
                    DurableAdmissionMessageKind::CancelRequested => 7,
                    DurableAdmissionMessageKind::Rejected => 8,
                },
                message_id: frame.message_id,
                predecessor_message_id: frame.predecessor_message_id,
                payload: frame.payload.clone(),
            })
        }
        PairingSessionMessage::Reject(r) => WireBody::Reject(WirePairingReject {
            reason: match &r.reason {
                PairingRejectReason::InvitationMismatch => WireRejectReason::InvitationMismatch,
                PairingRejectReason::AdmissionUnavailable => WireRejectReason::AdmissionUnavailable,
                PairingRejectReason::AdmissionConflict => WireRejectReason::AdmissionConflict,
                PairingRejectReason::PassphraseMismatch => WireRejectReason::PassphraseMismatch,
                PairingRejectReason::UserRejected => WireRejectReason::UserRejected,
                PairingRejectReason::Timeout => WireRejectReason::Timeout,
                PairingRejectReason::Internal(s) => WireRejectReason::Internal(s.clone()),
            },
        }),
    }
}

fn from_wire(body: WireBody) -> Result<PairingSessionMessage, WireDecodeError> {
    match body {
        WireBody::Request(r) => {
            let membership_credential = uc_core::membership::MembershipCredential::new(
                r.membership_credential.signature_algorithm_version,
                r.membership_credential.public_key,
            );
            if membership_credential.credential_format_version
                != r.membership_credential.credential_format_version
                || membership_credential.credential_id.as_bytes()
                    != &r.membership_credential.credential_id
            {
                return Err(WireDecodeError::InvalidDurableJoinRequest(
                    "membership credential is invalid".to_owned(),
                ));
            }
            let request = JoinerRequest {
                attempt_id: r.attempt_id,
                join_id: r.join_id,
                request_message_id: r.request_message_id,
                invitation_code: InvitationCode::new(r.invitation_code),
                device_id: DeviceId::new(r.device_id),
                device_name: r.device_name,
                identity_fingerprint: parse_fingerprint(&r.identity_fingerprint)?,
                nonce: r.nonce,
                transport_address_blob: r.transport_address_blob,
                security_capability: match r.security_capability {
                    1 => PairingSecurityCapability::ReliableGroupEpochV1,
                    other => return Err(WireDecodeError::UnsupportedSecurityCapability(other)),
                },
                key_package: r.key_package,
                member_instance: MemberInstanceId::from_bytes(r.member_instance),
                membership_credential,
                resume_public_key: r.resume_public_key,
                admission: AdmissionChangeFacts {
                    member_instance: MemberInstanceId::from_bytes(r.admission.member_instance),
                    device_id: DeviceId::new(r.admission.device_id),
                    device_name: r.admission.device_name,
                    identity_fingerprint: parse_fingerprint(&r.admission.identity_fingerprint)?,
                    transport_public_key: r.admission.transport_public_key,
                    transport_address_blob: r.admission.transport_address_blob,
                    identity_signature: r.admission.identity_signature,
                },
            };
            request
                .validate_durable_identity()
                .map_err(|error| WireDecodeError::InvalidDurableJoinRequest(error.to_owned()))?;
            Ok(PairingSessionMessage::Request(request))
        }
        WireBody::AdmissionOffer(o) => Ok(PairingSessionMessage::AdmissionOffer(
            SponsorAdmissionOffer {
                space_id: SpaceId::from_string(o.space_id),
                kdf_parameters_blob: o.kdf_parameters_blob,
                challenge: o.challenge,
                pairing_session_id: PairingSessionId::new(o.pairing_session_id),
            },
        )),
        WireBody::ChallengeResponse(c) => Ok(PairingSessionMessage::ChallengeResponse(
            JoinerChallengeResponse {
                encrypted_challenge: c.encrypted_challenge,
            },
        )),
        WireBody::DurableAdmission(frame) => Ok(PairingSessionMessage::DurableAdmission(
            DurableAdmissionFrame {
                attempt_id: frame.attempt_id,
                kind: match frame.kind {
                    1 => DurableAdmissionMessageKind::Candidate,
                    2 => DurableAdmissionMessageKind::Prepared,
                    3 => DurableAdmissionMessageKind::Commit,
                    4 => DurableAdmissionMessageKind::Applied,
                    5 => DurableAdmissionMessageKind::Complete,
                    6 => DurableAdmissionMessageKind::CompleteAck,
                    7 => DurableAdmissionMessageKind::CancelRequested,
                    8 => DurableAdmissionMessageKind::Rejected,
                    other => {
                        return Err(WireDecodeError::UnsupportedDurableAdmissionKind(other));
                    }
                },
                message_id: frame.message_id,
                predecessor_message_id: frame.predecessor_message_id,
                payload: frame.payload,
            },
        )),
        WireBody::Reject(r) => Ok(PairingSessionMessage::Reject(PairingReject {
            reason: match r.reason {
                WireRejectReason::InvitationMismatch => PairingRejectReason::InvitationMismatch,
                WireRejectReason::AdmissionUnavailable => PairingRejectReason::AdmissionUnavailable,
                WireRejectReason::AdmissionConflict => PairingRejectReason::AdmissionConflict,
                WireRejectReason::PassphraseMismatch => PairingRejectReason::PassphraseMismatch,
                WireRejectReason::UserRejected => PairingRejectReason::UserRejected,
                WireRejectReason::Timeout => PairingRejectReason::Timeout,
                WireRejectReason::Internal(s) => PairingRejectReason::Internal(s),
            },
        })),
    }
}

fn parse_fingerprint(s: &str) -> Result<IdentityFingerprint, WireDecodeError> {
    IdentityFingerprint::from_display_string(s)
        .map_err(|e| WireDecodeError::InvalidFingerprint(e.to_string()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fingerprint() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP").unwrap()
    }

    fn valid_wire_request() -> WireJoinerRequest {
        let device_id = DeviceId::new("device-a");
        let credential = uc_core::membership::MembershipCredential::new(1, vec![7; 32]);
        let member_instance = credential.member_instance_id(&device_id);
        WireJoinerRequest {
            attempt_id: [1; 32],
            join_id: [2; 16],
            request_message_id: [3; 32],
            invitation_code: "ABCDEFGH".into(),
            device_id: device_id.as_str().to_string(),
            device_name: "Device A".into(),
            identity_fingerprint: sample_fingerprint().as_display().to_string(),
            nonce: vec![1; 32],
            transport_address_blob: vec![2],
            security_capability: 1,
            key_package: vec![3],
            member_instance: *member_instance.as_bytes(),
            membership_credential: WireMembershipCredential {
                credential_format_version: credential.credential_format_version,
                signature_algorithm_version: credential.signature_algorithm_version,
                public_key: credential.public_key,
                credential_id: *credential.credential_id.as_bytes(),
            },
            resume_public_key: vec![8; 32],
            admission: WireAdmissionFacts {
                member_instance: *member_instance.as_bytes(),
                device_id: device_id.as_str().to_string(),
                device_name: "Device A".into(),
                identity_fingerprint: sample_fingerprint().as_display().to_string(),
                transport_public_key: vec![9; 32],
                transport_address_blob: vec![2],
                identity_signature: vec![10; 64],
            },
        }
    }

    fn round_trip(msg: PairingSessionMessage) -> PairingSessionMessage {
        let bytes = encode(&msg).expect("encode");
        decode(&bytes).expect("decode")
    }

    #[test]
    fn request_round_trips() {
        let membership_credential =
            uc_core::membership::MembershipCredential::new(1, vec![0x41; 32]);
        let member_instance = membership_credential.member_instance_id(&DeviceId::new("dev-001"));
        let admission = AdmissionChangeFacts {
            member_instance,
            device_id: DeviceId::new("dev-001"),
            device_name: "Alice's laptop".to_string(),
            identity_fingerprint: sample_fingerprint(),
            transport_public_key: vec![0x42; 32],
            transport_address_blob: vec![0x9a, 0x01, 0x02],
            identity_signature: vec![0x43; 64],
        };
        let original = PairingSessionMessage::Request(JoinerRequest {
            attempt_id: [0x11; 32],
            join_id: [0x12; 16],
            request_message_id: [0x13; 32],
            invitation_code: InvitationCode::new("CODE-1234"),
            device_id: DeviceId::new("dev-001"),
            device_name: "Alice's laptop".to_string(),
            identity_fingerprint: sample_fingerprint(),
            nonce: vec![1, 2, 3, 4, 5],
            transport_address_blob: vec![0x9a, 0x01, 0x02],
            security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
            key_package: vec![0x44, 0x55],
            member_instance,
            membership_credential: membership_credential.clone(),
            resume_public_key: vec![0x45; 32],
            admission: admission.clone(),
        });

        let decoded = round_trip(original);
        match decoded {
            PairingSessionMessage::Request(r) => {
                assert_eq!(r.attempt_id, [0x11; 32]);
                assert_eq!(r.join_id, [0x12; 16]);
                assert_eq!(r.request_message_id, [0x13; 32]);
                assert_eq!(r.invitation_code.as_str(), "CODE-1234");
                assert_eq!(r.device_id.as_str(), "dev-001");
                assert_eq!(r.device_name, "Alice's laptop");
                assert_eq!(r.identity_fingerprint, sample_fingerprint());
                assert_eq!(r.nonce, vec![1, 2, 3, 4, 5]);
                assert_eq!(r.transport_address_blob, vec![0x9a, 0x01, 0x02]);
                assert_eq!(
                    r.security_capability,
                    PairingSecurityCapability::ReliableGroupEpochV1
                );
                assert_eq!(r.key_package, vec![0x44, 0x55]);
                assert_eq!(r.member_instance, member_instance);
                assert_eq!(r.membership_credential, membership_credential);
                assert_eq!(r.resume_public_key, vec![0x45; 32]);
                assert_eq!(r.admission, admission);
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn request_rejects_member_instance_not_derived_from_credential() {
        let credential = uc_core::membership::MembershipCredential::new(1, vec![0x51; 32]);
        let device_id = DeviceId::new("dev-001");
        let wrong_instance = credential.member_instance_id(&DeviceId::new("another-device"));
        let body = WireBody::Request(WireJoinerRequest {
            attempt_id: [0x11; 32],
            join_id: [0x12; 16],
            request_message_id: [0x13; 32],
            invitation_code: "CODE-1234".to_string(),
            device_id: device_id.as_str().to_string(),
            device_name: "Alice's laptop".to_string(),
            identity_fingerprint: sample_fingerprint().as_display().to_string(),
            nonce: vec![],
            transport_address_blob: vec![0x61],
            security_capability: 1,
            key_package: vec![0x62],
            member_instance: *wrong_instance.as_bytes(),
            membership_credential: WireMembershipCredential {
                credential_format_version: credential.credential_format_version,
                signature_algorithm_version: credential.signature_algorithm_version,
                public_key: credential.public_key.clone(),
                credential_id: *credential.credential_id.as_bytes(),
            },
            resume_public_key: vec![0x63; 32],
            admission: WireAdmissionFacts {
                member_instance: *wrong_instance.as_bytes(),
                device_id: device_id.as_str().to_string(),
                device_name: "Alice's laptop".to_string(),
                identity_fingerprint: sample_fingerprint().as_display().to_string(),
                transport_public_key: vec![0x64; 32],
                transport_address_blob: vec![0x61],
                identity_signature: vec![0x65; 64],
            },
        });
        let mut bytes = vec![WIRE_VERSION];
        bytes.extend(postcard::to_stdvec(&body).unwrap());

        assert!(matches!(
            decode(&bytes),
            Err(WireDecodeError::InvalidDurableJoinRequest(_))
        ));
    }

    #[test]
    fn durable_admission_business_messages_round_trip_on_v10() {
        use uc_core::pairing::{DurableAdmissionFrame, DurableAdmissionMessageKind};

        for kind in [
            DurableAdmissionMessageKind::Candidate,
            DurableAdmissionMessageKind::Prepared,
            DurableAdmissionMessageKind::Commit,
            DurableAdmissionMessageKind::Applied,
            DurableAdmissionMessageKind::Complete,
            DurableAdmissionMessageKind::CompleteAck,
            DurableAdmissionMessageKind::CancelRequested,
            DurableAdmissionMessageKind::Rejected,
        ] {
            let original = PairingSessionMessage::DurableAdmission(DurableAdmissionFrame {
                attempt_id: [0x21; 32],
                kind,
                message_id: [0x22; 32],
                predecessor_message_id: Some([0x23; 32]),
                payload: vec![0x24, 0x25],
            });

            let decoded = round_trip(original);
            match decoded {
                PairingSessionMessage::DurableAdmission(frame) => {
                    assert_eq!(frame.attempt_id, [0x21; 32]);
                    assert_eq!(frame.kind, kind);
                    assert_eq!(frame.message_id, [0x22; 32]);
                    assert_eq!(frame.predecessor_message_id, Some([0x23; 32]));
                    assert_eq!(frame.payload, vec![0x24, 0x25]);
                }
                other => panic!("expected DurableAdmission, got {other:?}"),
            }
        }

        let encoded = encode(&PairingSessionMessage::DurableAdmission(
            DurableAdmissionFrame {
                attempt_id: [0x31; 32],
                kind: DurableAdmissionMessageKind::Complete,
                message_id: [0x32; 32],
                predecessor_message_id: None,
                payload: vec![0x33],
            },
        ))
        .unwrap();
        assert_eq!(encoded[0], 10, "V10 must be readable before body decoding");
    }

    #[test]
    fn pairing_wire_rejects_frames_over_four_mibibytes() {
        use uc_core::pairing::{DurableAdmissionFrame, DurableAdmissionMessageKind};

        let oversized = PairingSessionMessage::DurableAdmission(DurableAdmissionFrame {
            attempt_id: [0x71; 32],
            kind: DurableAdmissionMessageKind::Candidate,
            message_id: [0x72; 32],
            predecessor_message_id: None,
            payload: vec![0x73; super::MAX_FRAME_SIZE],
        });
        assert!(matches!(
            encode(&oversized),
            Err(WireEncodeError::FrameTooLarge { .. })
        ));

        let encoded = vec![0u8; super::MAX_FRAME_SIZE + 1];
        assert!(matches!(
            decode(&encoded),
            Err(WireDecodeError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn admission_offer_round_trips() {
        let original = PairingSessionMessage::AdmissionOffer(SponsorAdmissionOffer {
            space_id: SpaceId::from_str("space-42"),
            kdf_parameters_blob: vec![0xde, 0xad, 0xbe, 0xef],
            challenge: vec![0x01; 32],
            pairing_session_id: PairingSessionId::new("sess-abc-42"),
        });

        let decoded = round_trip(original);
        match decoded {
            PairingSessionMessage::AdmissionOffer(o) => {
                assert_eq!(o.space_id.inner(), "space-42");
                assert_eq!(o.kdf_parameters_blob, vec![0xde, 0xad, 0xbe, 0xef]);
                assert_eq!(o.challenge, vec![0x01; 32]);
                assert_eq!(o.pairing_session_id.as_str(), "sess-abc-42");
            }
            other => panic!("expected AdmissionOffer, got {other:?}"),
        }
    }

    #[test]
    fn challenge_response_round_trips() {
        let original = PairingSessionMessage::ChallengeResponse(JoinerChallengeResponse {
            encrypted_challenge: vec![0x42; 48],
        });
        let decoded = round_trip(original);
        match decoded {
            PairingSessionMessage::ChallengeResponse(c) => {
                assert_eq!(c.encrypted_challenge, vec![0x42; 48]);
            }
            other => panic!("expected ChallengeResponse, got {other:?}"),
        }
    }

    #[test]
    fn reject_round_trips_all_reasons() {
        for reason in [
            PairingRejectReason::InvitationMismatch,
            PairingRejectReason::AdmissionUnavailable,
            PairingRejectReason::AdmissionConflict,
            PairingRejectReason::PassphraseMismatch,
            PairingRejectReason::UserRejected,
            PairingRejectReason::Timeout,
            PairingRejectReason::Internal("bad things".to_string()),
        ] {
            let original = PairingSessionMessage::Reject(PairingReject {
                reason: reason.clone(),
            });
            let decoded = round_trip(original);
            match decoded {
                PairingSessionMessage::Reject(r) => assert_eq!(r.reason, reason),
                other => panic!("expected Reject, got {other:?}"),
            }
        }
    }

    #[test]
    fn decode_rejects_future_version() {
        // Build a forged envelope at v = WIRE_VERSION + 1 to verify
        // rejection semantics survive future bumps without touching this
        // test's hardcoded numbers.
        let fake_version = WIRE_VERSION + 1;
        let mut bytes = vec![fake_version];
        bytes.extend(
            postcard::to_allocvec(&WireBody::ChallengeResponse(WireJoinerChallengeResponse {
                encrypted_challenge: vec![],
            }))
            .unwrap(),
        );

        match decode(&bytes) {
            Err(WireDecodeError::UnsupportedVersion { got, expected }) => {
                assert_eq!(got, fake_version);
                assert_eq!(expected, WIRE_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_unsupported_security_capability() {
        let mut request = valid_wire_request();
        request.security_capability = 2;
        let mut bytes = vec![WIRE_VERSION];
        bytes.extend(postcard::to_allocvec(&WireBody::Request(request)).unwrap());

        assert!(matches!(
            decode(&bytes),
            Err(WireDecodeError::UnsupportedSecurityCapability(2))
        ));
    }

    #[test]
    fn decode_rejects_garbage_bytes() {
        let mut garbage = vec![WIRE_VERSION];
        garbage.extend([0xff; 15]);
        match decode(&garbage) {
            Err(WireDecodeError::Postcard(_)) => {}
            other => panic!("expected Postcard error, got {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_invalid_fingerprint_format() {
        // Manually build a request with a too-short fingerprint on the wire.
        let mut request = valid_wire_request();
        request.identity_fingerprint = "TOO_SHORT".to_string();
        let mut bytes = vec![WIRE_VERSION];
        bytes.extend(postcard::to_allocvec(&WireBody::Request(request)).unwrap());

        match decode(&bytes) {
            Err(WireDecodeError::InvalidFingerprint(msg)) => {
                assert!(
                    msg.contains("expected 16 characters"),
                    "unexpected error body: {msg}"
                );
            }
            other => panic!("expected InvalidFingerprint, got {other:?}"),
        }
    }

    #[test]
    fn encoded_payload_is_binary_and_nontrivial() {
        let msg = PairingSessionMessage::ChallengeResponse(JoinerChallengeResponse {
            encrypted_challenge: vec![1, 2, 3],
        });
        let bytes = encode(&msg).unwrap();
        assert!(!bytes.is_empty());
        // Envelope version byte should be the first byte for postcard's
        // layout of `struct { v: u8, body: enum }`.
        assert_eq!(bytes[0], WIRE_VERSION);
    }
}
