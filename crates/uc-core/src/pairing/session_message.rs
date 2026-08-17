//! Slice 1 pairing session-level domain messages.
//!
//! Pure domain types carried by [`PairingSessionPort`] and surfaced by
//! [`PairingEventPort`]. Adapters own wire encoding — these types have no
//! `serde` derives, no protocol ids, no libp2p / iroh leakage.
//!
//! Shape tracks the Slice 1 handshake:
//!
//! ```text
//!   Joiner → Sponsor : Request
//!   Sponsor → Joiner : KeyslotOffer
//!   Joiner → Sponsor : ChallengeResponse
//!   Sponsor → Joiner : Confirm
//!   Joiner → Sponsor : Ready        (or Reject at any step, either side)
//! ```
//!
//! Legacy libp2p-era equivalents live in [`crate::network::protocol::pairing`]
//! and carry a different — PIN-based, `peer_id`-leaky — shape. Slice 5 will
//! delete that module together with the libp2p adapter.
//!
//! [`PairingSessionPort`]: crate::ports::pairing::PairingSessionPort
//! [`PairingEventPort`]: crate::ports::pairing::PairingEventPort

use super::invitation::InvitationCode;
use crate::ids::{DeviceId, SpaceId};
use crate::membership::{AdmissionChangeFacts, MemberInstanceId, MembershipCredential};
use crate::ports::pairing::PairingSessionId;
use crate::security::IdentityFingerprint;

/// All pairing session-level messages for the Slice 1 iroh-native flow.
#[derive(Debug, Clone)]
pub enum PairingSessionMessage {
    Request(JoinerRequest),
    AdmissionOffer(SponsorAdmissionOffer),
    ChallengeResponse(JoinerChallengeResponse),
    DurableAdmission(DurableAdmissionFrame),
    Reject(PairingReject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableAdmissionMessageKind {
    Candidate,
    Prepared,
    Commit,
    Applied,
    Complete,
    CompleteAck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableAdmissionFrame {
    pub attempt_id: [u8; 32],
    pub kind: DurableAdmissionMessageKind,
    pub message_id: [u8; 32],
    pub predecessor_message_id: Option<[u8; 32]>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingSecurityCapability {
    ReliableGroupEpochV1,
}

/// Joiner → sponsor. First message on the bi-stream (B2 step 5).
#[derive(Debug, Clone)]
pub struct JoinerRequest {
    /// Stable private admission attempt identity, persisted before this
    /// request is sent.
    pub attempt_id: [u8; 32],
    /// Stable public join identity returned by product queries and cancel.
    pub join_id: [u8; 16],
    /// Identity of the durable JoinRequest record this frame delivers.
    pub request_message_id: [u8; 32],
    /// Code the joiner redeemed. Sponsor orchestrator matches it against
    /// the in-memory pending invitation (Q-B1-3 / F-041).
    pub invitation_code: InvitationCode,
    /// Joiner's stable business device id (F-036 concept 1).
    pub device_id: DeviceId,
    /// Joiner's device name for sponsor-side UI / persistence.
    pub device_name: String,
    /// Joiner's identity fingerprint (F-036 concept 2). Derived at the
    /// adapter from the Ed25519 pubkey used by the session's transport.
    pub identity_fingerprint: IdentityFingerprint,
    /// Handshake transcript nonce.
    pub nonce: Vec<u8>,
    /// 不透明传输地址 blob（Slice 2 Phase 1 · T5）。
    ///
    /// 由 joiner 端 adapter 用自身的 transport 编码（iroh adapter 用
    /// postcard 编码 `EndpointAddr`）。core 不解析内容，只把字节作为
    /// 透传字段交给 sponsor 端写入 [`PeerAddressRepositoryPort`]。
    /// 空 `Vec` 表示 joiner 端 adapter 无法提供地址（旧客户端或尚未
    /// publish direct addrs），sponsor 端降级为跳过 upsert。
    ///
    /// [`PeerAddressRepositoryPort`]: crate::ports::PeerAddressRepositoryPort
    pub transport_address_blob: Vec<u8>,
    /// Explicit fail-closed capability. Older pairing protocols cannot join a
    /// Space that has group epochs enabled.
    pub security_capability: PairingSecurityCapability,
    /// MLS KeyPackage. The matching private state never leaves the joiner.
    pub key_package: Vec<u8>,
    /// Exact member identity derived from the credential in this request.
    pub member_instance: MemberInstanceId,
    /// Public historical-verification credential for the proposed member.
    pub membership_credential: MembershipCredential,
    /// Public half of the durable retry identity saved with J0.
    pub resume_public_key: Vec<u8>,
    /// Member-signed facts that bind the request identity to the transport.
    pub admission: AdmissionChangeFacts,
}

impl JoinerRequest {
    pub fn validate_durable_identity(&self) -> Result<(), &'static str> {
        let canonical_credential = MembershipCredential::new(
            self.membership_credential.signature_algorithm_version,
            self.membership_credential.public_key.clone(),
        );
        if canonical_credential != self.membership_credential {
            return Err("membership credential is invalid");
        }
        if self
            .membership_credential
            .member_instance_id(&self.device_id)
            != self.member_instance
        {
            return Err("member instance does not match credential and device");
        }
        if self.admission.member_instance != self.member_instance
            || self.admission.device_id != self.device_id
            || self.admission.device_name != self.device_name
            || self.admission.identity_fingerprint != self.identity_fingerprint
            || self.admission.transport_address_blob != self.transport_address_blob
        {
            return Err("admission facts do not match request identity");
        }
        if self.attempt_id == [0; 32]
            || self.join_id == [0; 16]
            || self.request_message_id == [0; 32]
            || self.key_package.is_empty()
            || self.resume_public_key.len() != 32
            || self.admission.transport_public_key.is_empty()
            || self.admission.identity_signature.is_empty()
        {
            return Err("durable join request material is incomplete");
        }
        Ok(())
    }
}

/// Sponsor → joiner. Hands the joiner an offer they can unseal with the
/// shared passphrase (B2 step 6).
#[derive(Debug, Clone)]
pub struct SponsorAdmissionOffer {
    /// The space this offer belongs to.
    pub space_id: SpaceId,
    /// Opaque keyslot payload. Infra serializes the historical
    /// `KeySlotFile` JSON here; core treats the blob as bytes.
    pub kdf_parameters_blob: Vec<u8>,
    /// 32-byte challenge nonce the joiner combines with the derived
    /// master key and `pairing_session_id` to compute an HMAC proof
    /// ([`ProofPort::build_proof`](crate::ports::space::ProofPort)).
    /// Sponsor keeps a copy in per-session state and feeds the same
    /// value to `verify_proof` on receipt.
    pub challenge: Vec<u8>,
    /// Sponsor-minted session identifier replayed verbatim into the
    /// joiner's proof payload so the sponsor-side `verify_proof` can
    /// bind the HMAC to the live pairing session (replay defence).
    pub pairing_session_id: PairingSessionId,
}

/// Joiner → sponsor. Challenge decrypt proof (B2 step 8).
#[derive(Debug, Clone)]
pub struct JoinerChallengeResponse {
    pub encrypted_challenge: Vec<u8>,
}

/// Either side → other. Terminal message with a structured reason so the
/// orchestrator can pick the right UI error / `PairingError` variant.
#[derive(Debug, Clone)]
pub struct PairingReject {
    pub reason: PairingRejectReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingRejectReason {
    /// Sponsor: incoming code didn't match any pending invitation (stale
    /// rendezvous entry or attacker replay).
    InvitationMismatch,
    /// Sponsor: this space currently cannot admit a member. The reason is
    /// intentionally not exposed on the pairing channel.
    AdmissionUnavailable,
    /// Sponsor: the request conflicts with the sponsor's current durable
    /// membership history and cannot succeed by retrying the same request.
    AdmissionConflict,
    /// Sponsor: joiner's challenge response didn't decrypt — wrong
    /// passphrase.
    PassphraseMismatch,
    /// Sponsor: user declined (reserved; Slice 1 doesn't surface an
    /// approval prompt but the enum leaves room for it).
    UserRejected,
    /// Sponsor: handshake未在 TTL 内完成（`begin` 后既没看到 `confirm`
    /// 也没看到 `reject` / `close`）。与 `Internal(String)` 分开是
    /// 因为 timeout 是一个稳定、可观测的产品语义（UI 可以直接展示
    /// "配对超时"），不是字符串化的兜底错误。
    Timeout,
    /// Protocol-level violation; message is for logs only.
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_reason_equality_is_structural() {
        assert_eq!(
            PairingRejectReason::InvitationMismatch,
            PairingRejectReason::InvitationMismatch
        );
        assert_ne!(
            PairingRejectReason::Internal("a".into()),
            PairingRejectReason::Internal("b".into())
        );
    }
}
