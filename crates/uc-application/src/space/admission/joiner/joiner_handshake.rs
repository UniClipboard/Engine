//! Joiner-side handshake coordinator.
//!
//! Owns the transport + crypto half of the joiner-side pairing flow:
//! dial → send Request → recv AdmissionOffer → derive admission proof key → build
//! proof → send ChallengeResponse → recv Confirm. Returns a structured
//! [`JoinerHandshakeOutcome`] with the sponsor's identity facts. Does
//! **not** touch persistence (`SpaceMember` / `TrustedPeer` /
//! `SetupStatus`) — that's composed in the outer
//! [`RedeemPairingInvitationUseCase`].
//!
//! Symmetric to [`crate::pairing_inbound::sponsor_handshake::
//! SponsorHandshakeCoordinator`]:
//!
//! | concern                 | sponsor                    | joiner                      |
//! |-------------------------|----------------------------|-----------------------------|
//! | coordinator owns        | wire + `verify_proof`      | wire + `derive + build_proof` |
//! | stateful across events  | yes (parked `SessionCtx`)  | no (single-shot `handshake`) |
//! | TTL                     | spawned watchdog (P7g)     | per-`recv` `tokio::timeout` |
//! | close                   | coordinator drives         | coordinator drives          |
//! | persistence             | done by orchestrator       | done by use case            |
//!
//! ## Why this split exists
//!
//! Prior P7h landed everything in one 11-arg use case. The break from
//! sponsor-side shape made the use case a code smell (it did dial +
//! identity assembly + crypto + recv/decode + admit + trust +
//! setup-status). Extracting the coordinator brings joiner back in line
//! with sponsor architecture and drops the use case to 5 deps.
//!
//! ## Error type
//!
//! The coordinator returns [`RedeemPairingInvitationError`] directly
//! rather than a private enum: its variants 1-to-1 map user-facing
//! joiner failures, and the outer use case has no additional variants
//! to introduce at the seam. A private enum + map layer would be
//! duplication with zero signal gain.
//!
//! [`RedeemPairingInvitationError`]:
//!     crate::facade::space_setup::RedeemPairingInvitationError

use std::sync::Arc;

use tokio::time::{timeout, Duration};
use tracing::{debug, info, instrument, warn};

use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SessionId, SpaceId};
use uc_core::membership::{AdmissionChangeFacts, MemberInstanceId};
use uc_core::pairing::invitation::InvitationCode;
use uc_core::pairing::session_message::{
    JoinerChallengeResponse, JoinerRequest, PairingRejectReason, PairingSecurityCapability,
    PairingSessionMessage,
};
use uc_core::ports::pairing::{
    DialError, DialOutcome, DiscoveryChannel, PairingSessionId, PairingSessionPort, SessionError,
};
use uc_core::ports::space::{
    DeriveAdmissionProofKeyPort, GroupAdmissionPort, PrepareAdmissionTargetAccessPort, ProofPort,
    SpaceAccessError,
};
use uc_core::ports::{DeviceIdentityPort, LocalIdentityPort, SettingsPort};
use uc_core::security::IdentityFingerprint;
use uc_core::space_access::AdmissionOffer;

use crate::facade::space_setup::RedeemPairingInvitationError;
use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;

/// Facts handed to the use case after a successful joiner-side handshake.
///
/// Shaped for the subsequent persistence step (`admit` + `trust`) plus
/// the UI confirmation surface (`self_*` fields) the use case returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinerHandshakeOutcome {
    pub sponsor_device_id: DeviceId,
    pub sponsor_device_name: String,
    pub sponsor_identity_fingerprint: IdentityFingerprint,
    pub space_id: SpaceId,
    pub self_device_id: DeviceId,
    pub self_identity_fingerprint: IdentityFingerprint,
    /// Discovery channel that resolved the invitation before the dial
    /// (cloud directory vs LAN). Carried so the outer use case can record
    /// which channel the first pair actually used.
    pub discovery_channel: DiscoveryChannel,
    /// Sponsor 准入完成消息带来的不透明传输地址字节，由 outer use case
    /// best-effort upsert 到
    /// `PeerAddressRepositoryPort`。空 `Vec` 表示 sponsor 未附带地址，
    /// joiner 侧应跳过 upsert。
    pub sponsor_transport_address_blob: Vec<u8>,
    /// Phase 098：sponsor 派发的 telemetry person 标识。
    ///
    /// `Some(uuid)`：joiner 端 use case 在 `pairing_succeeded` 之前调
    /// `analytics_identity.adopt_space_person(uuid)` 接受这个 ID 并发
    /// `$identify`，让本机 telemetry 与 sponsor 聚合为同一 person。
    ///
    /// `None`：sponsor 端尚未确立 telemetry 身份（v1→v2 升级未配对场景）。
    /// joiner 端按 Solo 退化，等待下次 sponsor 自己发新设备 pairing 时再
    /// 通过 sponsor 派发统一切换（task_plan §开放问题 2 决策 A）。
    pub sponsor_space_person_id: Option<uuid::Uuid>,
    /// The member instance derived from this admission's freshly generated
    /// credential. The joiner must identify itself by this instance even
    /// when its security view still carries a stale instance from an
    /// earlier admission of the same device.
    pub member_instance: Option<MemberInstanceId>,
}

#[derive(Debug)]
pub(crate) struct PendingJoinerHandshake {
    pub(crate) session: PairingSessionId,
    pub(crate) outcome: JoinerHandshakeOutcome,
    requires_session_transition: bool,
}

impl PendingJoinerHandshake {
    pub(crate) fn outcome(&self) -> &JoinerHandshakeOutcome {
        &self.outcome
    }

    pub(crate) fn requires_session_transition(&self) -> bool {
        self.requires_session_transition
    }
}

pub(crate) struct JoinerHandshakeCoordinator {
    pairing_session: Arc<dyn PairingSessionPort>,
    space_access: Arc<dyn DeriveAdmissionProofKeyPort>,
    target_access: Arc<dyn PrepareAdmissionTargetAccessPort>,
    group_admission: Arc<dyn GroupAdmissionPort>,
    proof_port: Arc<dyn ProofPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    settings: Arc<dyn SettingsPort>,
    workspace_convergence: Arc<dyn WorkspaceAdmissionOwnerPort>,
    /// Per-`recv` TTL — not end-to-end handshake TTL. Independent of
    /// P7g's sponsor-side watchdog: this timer protects against silent
    /// sponsor, the sponsor's protects against silent joiner.
    handshake_ttl: Duration,
}

impl JoinerHandshakeCoordinator {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        pairing_session: Arc<dyn PairingSessionPort>,
        space_access: Arc<dyn DeriveAdmissionProofKeyPort>,
        target_access: Arc<dyn PrepareAdmissionTargetAccessPort>,
        group_admission: Arc<dyn GroupAdmissionPort>,
        proof_port: Arc<dyn ProofPort>,
        local_identity: Arc<dyn LocalIdentityPort>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
        workspace_convergence: Arc<dyn WorkspaceAdmissionOwnerPort>,
        handshake_ttl: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            pairing_session,
            space_access,
            target_access,
            group_admission,
            proof_port,
            local_identity,
            device_identity,
            settings,
            workspace_convergence,
            handshake_ttl,
        })
    }

    /// Run the full wire + crypto flow. On success the session has
    /// been closed cleanly and the outcome is ready for the outer use
    /// case to persist. On failure the session is also closed (so the
    /// adapter releases its slot) and the error is surfaced.
    #[instrument(skip_all, fields(code = %code.as_str()))]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn handshake(
        &self,
        code: &InvitationCode,
        passphrase: &Passphrase,
    ) -> Result<PendingJoinerHandshake, RedeemPairingInvitationError> {
        self.handshake_with_history_policy(code, passphrase, false)
            .await
    }

    pub(crate) async fn handshake_with_history_policy(
        &self,
        code: &InvitationCode,
        passphrase: &Passphrase,
        preserve_unreadable_history: bool,
    ) -> Result<PendingJoinerHandshake, RedeemPairingInvitationError> {
        self.workspace_convergence
            .preflight_local_join_source(preserve_unreadable_history)
            .await
            .map_err(map_workspace_preflight_error)?;
        // The source-history policy is resolved before this point so a
        // required confirmation never contacts the sponsor. A successful dial creates
        // a session in the adapter that we must close on every exit
        // path below (including error paths).
        let DialOutcome {
            session_id: session,
            channel,
        } = self
            .pairing_session
            .dial_by_invitation(code)
            .await
            .map_err(map_dial_err)?;
        info!(session = %session, ?channel, "pairing session dialled");

        match self
            .drive(
                &session,
                channel,
                code,
                passphrase,
                preserve_unreadable_history,
            )
            .await
        {
            Ok(pending) => Ok(pending),
            Err(err) => {
                self.pairing_session
                    .close(&session, Some(format!("handshake aborted: {err}")))
                    .await;
                Err(err)
            }
        }
    }

    pub(crate) async fn abort(&self, pending: PendingJoinerHandshake, error: &str) {
        self.pairing_session
            .close(
                &pending.session,
                Some(format!("joiner persistence failed: {error}")),
            )
            .await;
    }

    async fn drive(
        &self,
        session: &PairingSessionId,
        channel: DiscoveryChannel,
        code: &InvitationCode,
        passphrase: &Passphrase,
        preserve_unreadable_history: bool,
    ) -> Result<PendingJoinerHandshake, RedeemPairingInvitationError> {
        // ── 1. Collect local facts ───────────────────────────────────────
        let local_fp = self.local_identity.ensure().await.map_err(|e| {
            RedeemPairingInvitationError::Internal(format!("local_identity.ensure: {e}"))
        })?;
        let local_device_id = self.device_identity.current_device_id();
        let local_device_name = self
            .settings
            .load()
            .await
            .map_err(|e| RedeemPairingInvitationError::Internal(format!("settings.load: {e}")))?
            .general
            .device_name
            .filter(|n| !n.trim().is_empty())
            .ok_or(RedeemPairingInvitationError::DeviceNameRequired)?;
        info!(
            session = %session,
            local_device_id = %local_device_id.as_str(),
            "joiner pairing local facts loaded"
        );

        // ── 2. Send JoinerRequest ────────────────────────────────────────
        // Slice 2 Phase 1 · T5：adapter 暴露本机传输地址 blob；`None`
        // 兜底为空 Vec —— sponsor 收到空 blob 就跳过 peer address upsert，
        // 不阻塞配对本身。
        let transport_address_blob = self
            .pairing_session
            .local_transport_address_blob()
            .await
            .unwrap_or_default();
        let transport_address_blob_len = transport_address_blob.len();
        let stable_request_binding = crate::space::admission::adapter::stable_join_request_binding(
            &local_device_id,
            &local_fp,
        );
        let durable = self
            .workspace_convergence
            .prepare_local_join_before_network(
                self.group_admission.as_ref(),
                &local_device_id,
                code.as_str().as_bytes(),
                &stable_request_binding,
                preserve_unreadable_history,
            )
            .await
            .map_err(map_local_join_preparation_error)?;
        let pending_group_join = durable.prepared_group_join;
        let membership_credential = self
            .group_admission
            .prepared_join_membership_credential(&pending_group_join)
            .await
            .map_err(map_space_access_err)?;
        let member_instance = membership_credential.member_instance_id(&local_device_id);
        if pending_group_join.member_instance() != Some(member_instance) {
            return Err(RedeemPairingInvitationError::CorruptedKeyMaterial);
        }
        let transport_public_key = self
            .pairing_session
            .local_transport_public_key()
            .await
            .ok_or_else(|| {
                RedeemPairingInvitationError::Internal(
                    "local transport public key is unavailable".to_owned(),
                )
            })?;
        let mut admission = AdmissionChangeFacts {
            member_instance,
            device_id: local_device_id.clone(),
            device_name: local_device_name.clone(),
            identity_fingerprint: local_fp.clone(),
            transport_public_key,
            transport_address_blob: transport_address_blob.clone(),
            identity_signature: Vec::new(),
        };
        admission.identity_signature = self
            .group_admission
            .sign_prepared_join_payload(&pending_group_join, &admission.signing_payload())
            .await
            .map_err(map_space_access_err)?;
        let request = JoinerRequest {
            attempt_id: durable.attempt_id,
            join_id: durable.join_id,
            request_message_id: durable.request_message_id,
            invitation_code: code.clone(),
            device_id: local_device_id,
            device_name: local_device_name,
            identity_fingerprint: local_fp.clone(),
            // 保留字段：Slice 1 sponsor 不消费 transcript nonce，留空占位
            // 即可；加 rand crate 不值当。未来 slice 若把 transcript
            // binding 纳入 HMAC，再在这里填。
            nonce: Vec::new(),
            transport_address_blob,
            security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
            key_package: pending_group_join.key_package.clone(),
            member_instance,
            membership_credential,
            resume_public_key: durable.resume_public_key,
            admission,
        };
        self.pairing_session
            .send(session, PairingSessionMessage::Request(request))
            .await
            .map_err(map_session_err)?;
        info!(
            session = %session,
            code = %code.as_str(),
            transport_address_blob_len,
            "JoinerRequest sent; awaiting AdmissionOffer"
        );

        // ── 3. Await AdmissionOffer | Reject ───────────────────────────────
        let offer = match self.recv_with_ttl(session).await? {
            PairingSessionMessage::AdmissionOffer(o) => o,
            PairingSessionMessage::Reject(r) => {
                warn!(
                    session = %session,
                    reason = ?r.reason,
                    "sponsor rejected before AdmissionOffer"
                );
                self.workspace_convergence
                    .reject_local_join_before_candidate(
                        durable.attempt_id,
                        durable_rejection_reason(&r.reason),
                    )
                    .await
                    .map_err(|error| {
                        RedeemPairingInvitationError::Internal(format!(
                            "persist sponsor rejection before AdmissionOffer: {error}"
                        ))
                    })?;
                return Err(map_sponsor_reject(r.reason));
            }
            other => {
                return Err(RedeemPairingInvitationError::Internal(format!(
                    "expected AdmissionOffer, got {}",
                    variant_name(&other),
                )));
            }
        };
        debug!(
            session = %session,
            space_id = %offer.space_id,
            "AdmissionOffer received"
        );

        // ── 4. Derive proof key (side effect: persists local keyslot) ───
        let challenge_nonce = challenge_to_array(&offer.challenge)?;
        // Pairing session identifiers are local to each endpoint. The
        // sponsor sends its identifier so both sides bind the proof to the
        // same value; it is not expected to equal the joiner's dial handle.
        let core_session = SessionId::new(offer.pairing_session_id.as_str().to_string());
        let join_offer = AdmissionOffer {
            space_id: offer.space_id.clone(),
            kdf_parameters_blob: offer.kdf_parameters_blob.clone(),
            challenge_nonce,
        };
        let derived_key = self
            .space_access
            .derive_admission_proof_key(&join_offer, passphrase, code, &core_session)
            .await
            .map_err(map_space_access_err)?;
        debug!(session = %session, "admission proof key derived from sponsor offer");

        // ── 5. Build HMAC proof ──────────────────────────────────────────
        let proof = self
            .proof_port
            .build_proof(
                &core_session,
                &join_offer.space_id,
                challenge_nonce,
                &derived_key,
            )
            .await
            .map_err(|e| RedeemPairingInvitationError::Internal(format!("build_proof: {e}")))?;

        // ── 6. Send ChallengeResponse ────────────────────────────────────
        self.pairing_session
            .send(
                session,
                PairingSessionMessage::ChallengeResponse(JoinerChallengeResponse {
                    encrypted_challenge: proof.proof_bytes,
                }),
            )
            .await
            .map_err(map_session_err)?;
        info!(session = %session, "ChallengeResponse sent; awaiting Candidate/Reject");

        let candidate = match self.recv_with_ttl(session).await? {
            PairingSessionMessage::DurableAdmission(frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::Candidate =>
            {
                frame
            }
            PairingSessionMessage::Reject(r) => {
                warn!(
                    session = %session,
                    reason = ?r.reason,
                    "sponsor rejected before Candidate"
                );
                self.workspace_convergence
                    .reject_local_join_before_candidate(
                        durable.attempt_id,
                        durable_rejection_reason(&r.reason),
                    )
                    .await
                    .map_err(|error| {
                        RedeemPairingInvitationError::Internal(format!(
                            "persist sponsor rejection before Candidate: {error}"
                        ))
                    })?;
                return Err(map_sponsor_reject(r.reason));
            }
            other => {
                return Err(RedeemPairingInvitationError::Internal(format!(
                    "expected Candidate, got {}",
                    variant_name(&other),
                )));
            }
        };
        let candidate_payload =
            crate::space::convergence::admission::DurableAdmissionCandidatePayloadV1::decode(
                &candidate.payload,
            )
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!("decode Candidate: {error}"))
            })?;
        if candidate_payload.candidate.lineage_id != join_offer.space_id.as_ref() {
            return Err(RedeemPairingInvitationError::CorruptedKeyMaterial);
        }
        let candidate_event: uc_core::membership::MembershipEventV2 = postcard::from_bytes(
            &candidate_payload.candidate.candidate_event,
        )
        .map_err(|error| {
            RedeemPairingInvitationError::Internal(format!("decode candidate event: {error}"))
        })?;
        let sponsor = candidate_payload
            .candidate
            .target_relationships
            .iter()
            .find(|facts| facts.member_instance == candidate_event.author_member_instance_id)
            .cloned()
            .ok_or(RedeemPairingInvitationError::CorruptedKeyMaterial)?;
        let prepared = self
            .workspace_convergence
            .prepare_joiner_candidate(
                &candidate,
                self.group_admission.as_ref(),
                self.target_access.as_ref(),
                passphrase,
            )
            .await
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!("prepare Candidate: {error}"))
            })?;
        self.pairing_session
            .send(session, PairingSessionMessage::DurableAdmission(prepared))
            .await
            .map_err(map_session_err)?;
        let commit = match self.recv_with_ttl(session).await? {
            PairingSessionMessage::DurableAdmission(frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::Commit =>
            {
                frame
            }
            PairingSessionMessage::Reject(r) => return Err(map_sponsor_reject(r.reason)),
            other => {
                return Err(RedeemPairingInvitationError::Internal(format!(
                    "expected Commit, got {}",
                    variant_name(&other),
                )));
            }
        };
        let applied = self
            .workspace_convergence
            .apply_joiner_commit(&commit, self.group_admission.as_ref())
            .await
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!("apply Commit: {error}"))
            })?;
        self.pairing_session
            .send(session, PairingSessionMessage::DurableAdmission(applied))
            .await
            .map_err(map_session_err)?;
        let complete = match self.recv_with_ttl(session).await? {
            PairingSessionMessage::DurableAdmission(frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::Complete =>
            {
                frame
            }
            PairingSessionMessage::Reject(r) => return Err(map_sponsor_reject(r.reason)),
            other => {
                return Err(RedeemPairingInvitationError::Internal(format!(
                    "expected Complete, got {}",
                    variant_name(&other),
                )));
            }
        };
        let completion = self
            .workspace_convergence
            .activate_joiner_complete(&complete)
            .await
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!("activate Complete: {error}"))
            })?;
        let requires_session_transition = match completion {
            crate::space::admission::adapter::DurableJoinerCompletion::Active(complete_ack) => {
                self.pairing_session
                    .send(
                        session,
                        PairingSessionMessage::DurableAdmission(complete_ack),
                    )
                    .await
                    .map_err(map_session_err)?;
                info!(
                    session = %session,
                    sponsor_device_id = %sponsor.device_id.as_str(),
                    space_id = %join_offer.space_id,
                    "durable admission completed and acknowledged"
                );
                false
            }
            crate::space::admission::adapter::DurableJoinerCompletion::SpaceTransitionRequired => {
                info!(
                    session = %session,
                    sponsor_device_id = %sponsor.device_id.as_str(),
                    space_id = %join_offer.space_id,
                    "durable admission saved; session transition required"
                );
                true
            }
        };

        Ok(PendingJoinerHandshake {
            session: session.clone(),
            outcome: JoinerHandshakeOutcome {
                sponsor_device_id: sponsor.device_id,
                sponsor_device_name: sponsor.device_name,
                sponsor_identity_fingerprint: sponsor.identity_fingerprint,
                space_id: join_offer.space_id,
                discovery_channel: channel,
                self_device_id: local_device_id,
                self_identity_fingerprint: local_fp,
                sponsor_transport_address_blob: sponsor.transport_address_blob,
                sponsor_space_person_id: None,
                member_instance: Some(member_instance),
            },
            requires_session_transition,
        })
    }

    async fn recv_with_ttl(
        &self,
        session: &PairingSessionId,
    ) -> Result<PairingSessionMessage, RedeemPairingInvitationError> {
        match timeout(self.handshake_ttl, self.pairing_session.recv_next(session)).await {
            Err(_elapsed) => {
                warn!(
                    session = %session,
                    ttl_ms = %self.handshake_ttl.as_millis(),
                    "recv_with_ttl exceeded; aborting handshake"
                );
                Err(RedeemPairingInvitationError::Timeout)
            }
            Ok(Ok(Some(msg))) => {
                info!(
                    session = %session,
                    message_kind = variant_name(&msg),
                    "joiner pairing message received"
                );
                Ok(msg)
            }
            Ok(Ok(None)) => {
                warn!(session = %session, "joiner pairing session closed by sponsor");
                Err(RedeemPairingInvitationError::ConnectionLost)
            }
            Ok(Err(err)) => {
                warn!(
                    session = %session,
                    error = %err,
                    "joiner pairing recv failed"
                );
                Err(map_session_err(err))
            }
        }
    }
}

fn challenge_to_array(bytes: &[u8]) -> Result<[u8; 32], RedeemPairingInvitationError> {
    bytes.try_into().map_err(|_| {
        RedeemPairingInvitationError::Internal(format!(
            "challenge nonce wire length invalid: expected 32 bytes, got {}",
            bytes.len()
        ))
    })
}

fn map_dial_err(err: DialError) -> RedeemPairingInvitationError {
    match err {
        DialError::InvitationNotFound => RedeemPairingInvitationError::InvitationNotFound,
        DialError::InvitationExpired => RedeemPairingInvitationError::InvitationExpired,
        DialError::SponsorUnreachable => RedeemPairingInvitationError::SponsorUnreachable,
        DialError::ServiceUnavailable => RedeemPairingInvitationError::ServiceUnavailable,
        DialError::SponsorUpgradeRequired => RedeemPairingInvitationError::SponsorUpgradeRequired,
        DialError::Internal(m) => RedeemPairingInvitationError::Internal(m),
    }
}

fn map_session_err(err: SessionError) -> RedeemPairingInvitationError {
    match err {
        SessionError::NotFound(_) | SessionError::Closed => {
            RedeemPairingInvitationError::ConnectionLost
        }
        SessionError::Internal(m) => RedeemPairingInvitationError::Internal(m),
    }
}

fn map_space_access_err(err: SpaceAccessError) -> RedeemPairingInvitationError {
    match err {
        SpaceAccessError::WrongPassphrase => RedeemPairingInvitationError::PassphraseMismatch,
        SpaceAccessError::CorruptedKeyMaterial => {
            RedeemPairingInvitationError::CorruptedKeyMaterial
        }
        other => {
            RedeemPairingInvitationError::Internal(format!("derive_master_key_for_proof: {other}"))
        }
    }
}

fn map_sponsor_reject(reason: PairingRejectReason) -> RedeemPairingInvitationError {
    match reason {
        PairingRejectReason::InvitationMismatch => {
            RedeemPairingInvitationError::SponsorRejectedInvitation
        }
        PairingRejectReason::AdmissionUnavailable => {
            RedeemPairingInvitationError::SponsorAdmissionUnavailable
        }
        PairingRejectReason::AdmissionConflict => {
            RedeemPairingInvitationError::SponsorAdmissionConflict
        }
        // Sponsor's `verify_proof` failed = wrong passphrase — same
        // user-facing meaning as local `WrongPassphrase`, fold into one
        // variant so UI doesn't need to distinguish "who noticed first".
        PairingRejectReason::PassphraseMismatch => RedeemPairingInvitationError::PassphraseMismatch,
        PairingRejectReason::UserRejected => RedeemPairingInvitationError::SponsorDeclined,
        PairingRejectReason::Timeout => RedeemPairingInvitationError::SponsorTimedOut,
        PairingRejectReason::Internal(m) => RedeemPairingInvitationError::SponsorInternal(m),
    }
}

fn durable_rejection_reason(
    reason: &PairingRejectReason,
) -> uc_core::membership::AdmissionRejectionReasonV1 {
    use uc_core::membership::AdmissionRejectionReasonV1;

    match reason {
        PairingRejectReason::InvitationMismatch | PairingRejectReason::AdmissionUnavailable => {
            AdmissionRejectionReasonV1::InvitationUnavailable
        }
        PairingRejectReason::AdmissionConflict => AdmissionRejectionReasonV1::HistoryConflict,
        PairingRejectReason::PassphraseMismatch => {
            AdmissionRejectionReasonV1::AuthenticationRejected
        }
        PairingRejectReason::UserRejected
        | PairingRejectReason::Timeout
        | PairingRejectReason::Internal(_) => AdmissionRejectionReasonV1::Cancelled,
    }
}

fn map_workspace_preflight_error(
    error: crate::space::convergence::WorkspaceConvergenceError,
) -> RedeemPairingInvitationError {
    match error {
        crate::space::convergence::WorkspaceConvergenceError::UnreadableHistoryRequiresConfirmation => {
            RedeemPairingInvitationError::UnreadableHistoryRequiresConfirmation
        }
        crate::space::convergence::WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded => {
            RedeemPairingInvitationError::PreviousJoinCannotBeSuperseded
        }
        other => RedeemPairingInvitationError::Internal(format!(
            "preflight local join source: {other}"
        )),
    }
}

fn map_local_join_preparation_error(
    error: crate::space::convergence::WorkspaceConvergenceError,
) -> RedeemPairingInvitationError {
    match error {
        crate::space::convergence::WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded => {
            RedeemPairingInvitationError::PreviousJoinCannotBeSuperseded
        }
        other => RedeemPairingInvitationError::Internal(format!(
            "persist local join before network: {other}"
        )),
    }
}

fn variant_name(message: &PairingSessionMessage) -> &'static str {
    match message {
        PairingSessionMessage::Request(_) => "Request",
        PairingSessionMessage::AdmissionOffer(_) => "AdmissionOffer",
        PairingSessionMessage::ChallengeResponse(_) => "ChallengeResponse",
        PairingSessionMessage::DurableAdmission(_) => "DurableAdmission",
        PairingSessionMessage::Reject(_) => "Reject",
    }
}

#[cfg(test)]
mod tests {
    //! Wire + crypto tests live here. Composition (admit → trust →
    //! setup-status ordering) belongs to
    //! [`crate::space::admission::redeem_invitation::tests`].
    use super::*;
    use crate::space::admission::adapter::stable_join_request_binding;

    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use chrono::Duration as ChronoDuration;

    use uc_core::crypto::domain::Passphrase;
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::pairing::session_message::{
        JoinerChallengeResponse, JoinerRequest, PairingReject, SponsorAdmissionOffer,
    };
    use uc_core::ports::pairing::{DialError, DialOutcome, DiscoveryChannel, SessionError};
    use uc_core::ports::space::{
        DeriveAdmissionProofKeyPort, GroupAdmissionPort, SpaceAccessError,
    };
    use uc_core::ports::LocalIdentityError;
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;
    use uc_core::space_access::domain::{
        GroupAdmission, PreparedGroupJoin, ProofDerivedKey, SpaceAccessProofArtifact,
    };

    // ── fakes ────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct ScriptedSession {
        dial_result: StdMutex<Option<Result<PairingSessionId, DialError>>>,
        dial_calls: std::sync::atomic::AtomicUsize,
        sent: StdMutex<Vec<(PairingSessionId, PairingSessionMessage)>>,
        closed: StdMutex<Vec<(PairingSessionId, Option<String>)>>,
        recv_script: StdMutex<VecDeque<RecvStep>>,
        send_next_error: StdMutex<Option<SessionError>>,
    }
    enum RecvStep {
        Msg(PairingSessionMessage),
        CleanClose,
        Err(SessionError),
        /// "Never responds" — `std::future::pending().await` lets the
        /// caller's `tokio::time::timeout` wrapper fire under paused
        /// clock.
        Hang,
    }

    impl ScriptedSession {
        fn with_dial_ok(id: &str) -> Self {
            let me = Self::default();
            *me.dial_result.lock().unwrap() = Some(Ok(PairingSessionId::new(id.to_string())));
            me
        }
        fn with_dial_err(err: DialError) -> Self {
            let me = Self::default();
            *me.dial_result.lock().unwrap() = Some(Err(err));
            me
        }
        fn push_recv(&self, step: RecvStep) {
            self.recv_script.lock().unwrap().push_back(step);
        }
        fn sent(&self) -> Vec<(PairingSessionId, PairingSessionMessage)> {
            self.sent.lock().unwrap().clone()
        }
        fn closed(&self) -> Vec<(PairingSessionId, Option<String>)> {
            self.closed.lock().unwrap().clone()
        }
        fn dial_calls(&self) -> usize {
            self.dial_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl PairingSessionPort for ScriptedSession {
        async fn dial_by_invitation(
            &self,
            _code: &InvitationCode,
        ) -> Result<DialOutcome, DialError> {
            self.dial_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.dial_result.lock().unwrap().as_ref() {
                Some(Ok(id)) => Ok(DialOutcome {
                    session_id: id.clone(),
                    channel: DiscoveryChannel::Cloud,
                }),
                Some(Err(err)) => Err(clone_dial_err(err)),
                None => Err(DialError::Internal("test misconfigured".into())),
            }
        }
        async fn send(
            &self,
            session: &PairingSessionId,
            message: PairingSessionMessage,
        ) -> Result<(), SessionError> {
            if let Some(err) = self.send_next_error.lock().unwrap().take() {
                return Err(err);
            }
            self.sent.lock().unwrap().push((session.clone(), message));
            Ok(())
        }
        async fn recv_next(
            &self,
            _session: &PairingSessionId,
        ) -> Result<Option<PairingSessionMessage>, SessionError> {
            let next = self.recv_script.lock().unwrap().pop_front();
            match next {
                Some(RecvStep::Msg(m)) => Ok(Some(m)),
                Some(RecvStep::CleanClose) => Ok(None),
                Some(RecvStep::Err(e)) => Err(e),
                Some(RecvStep::Hang) | None => std::future::pending().await,
            }
        }
        async fn close(&self, session: &PairingSessionId, reason: Option<String>) {
            self.closed.lock().unwrap().push((session.clone(), reason));
        }

        async fn local_transport_public_key(&self) -> Option<Vec<u8>> {
            Some(vec![0x71; 32])
        }
    }

    fn clone_dial_err(err: &DialError) -> DialError {
        match err {
            DialError::InvitationNotFound => DialError::InvitationNotFound,
            DialError::InvitationExpired => DialError::InvitationExpired,
            DialError::SponsorUnreachable => DialError::SponsorUnreachable,
            DialError::ServiceUnavailable => DialError::ServiceUnavailable,
            DialError::SponsorUpgradeRequired => DialError::SponsorUpgradeRequired,
            DialError::Internal(m) => DialError::Internal(m.clone()),
        }
    }

    mockall::mock! {
        SpaceAccess {}

        #[async_trait]
        impl DeriveAdmissionProofKeyPort for SpaceAccess {
            async fn derive_admission_proof_key(
                &self,
                offer: &AdmissionOffer,
                passphrase: &Passphrase,
                invitation: &InvitationCode,
                pairing_session_id: &SessionId,
            ) -> Result<ProofDerivedKey, SpaceAccessError>;
        }
        #[async_trait]
        impl PrepareAdmissionTargetAccessPort for SpaceAccess {
            async fn prepare_target_access(
                &self,
                target_space_id: &SpaceId,
                passphrase: &Passphrase,
            ) -> Result<uc_core::space_access::PreparedAdmissionTargetAccess, SpaceAccessError>;
        }

        #[async_trait]
        impl GroupAdmissionPort for SpaceAccess {
            async fn prepare_group_join(
                &self,
                device_id: &DeviceId,
            ) -> Result<PreparedGroupJoin, SpaceAccessError>;
            async fn prepared_join_membership_credential(
                &self,
                pending: &PreparedGroupJoin,
            ) -> Result<uc_core::membership::MembershipCredential, SpaceAccessError>;
            async fn sign_prepared_join_payload(
                &self,
                pending: &PreparedGroupJoin,
                payload: &[u8],
            ) -> Result<Vec<u8>, SpaceAccessError>;
            async fn admit_group_member(
                &self,
                space_id: &SpaceId,
                sponsor_device_id: &DeviceId,
                joiner_device_id: &DeviceId,
                existing_member_ids: &[DeviceId],
                key_package: &[u8],
            ) -> Result<GroupAdmission, SpaceAccessError>;
            async fn install_group_join(
                &self,
                space_id: &SpaceId,
                passphrase: &Passphrase,
                pending: PreparedGroupJoin,
                welcome: &[u8],
                encrypted_key_catalog: &[u8],
                group_epoch: u64,
            ) -> Result<(), SpaceAccessError>;
        }
    }

    fn space_access() -> Arc<MockSpaceAccess> {
        let mut mock = MockSpaceAccess::new();
        mock.expect_prepare_group_join().returning(|device_id| {
            let credential = uc_core::membership::MembershipCredential::new(1, vec![0x72; 32]);
            Ok(PreparedGroupJoin::new(vec![1, 2, 3], vec![4, 5, 6])
                .with_member_instance(credential.member_instance_id(device_id)))
        });
        mock.expect_prepared_join_membership_credential()
            .returning(|_| {
                Ok(uc_core::membership::MembershipCredential::new(
                    1,
                    vec![0x72; 32],
                ))
            });
        mock.expect_sign_prepared_join_payload()
            .returning(|_, _| Ok(vec![0x73; 64]));
        mock.expect_derive_admission_proof_key()
            .returning(|_, _, _, _| Ok(ProofDerivedKey::from_bytes([0xCC; 32])));
        mock.expect_prepare_target_access().returning(|_, _| {
            Ok(
                uc_core::space_access::PreparedAdmissionTargetAccess::from_bytes(
                    b"target-access".to_vec(),
                ),
            )
        });
        mock.expect_install_group_join()
            .returning(|_, _, _, _, _, _| Ok(()));
        Arc::new(mock)
    }

    fn space_access_with_derivation_error(err: SpaceAccessError) -> Arc<MockSpaceAccess> {
        let mut mock = MockSpaceAccess::new();
        mock.expect_prepare_group_join().returning(|device_id| {
            let credential = uc_core::membership::MembershipCredential::new(1, vec![0x72; 32]);
            Ok(PreparedGroupJoin::new(vec![1, 2, 3], vec![4, 5, 6])
                .with_member_instance(credential.member_instance_id(device_id)))
        });
        mock.expect_prepared_join_membership_credential()
            .returning(|_| {
                Ok(uc_core::membership::MembershipCredential::new(
                    1,
                    vec![0x72; 32],
                ))
            });
        mock.expect_sign_prepared_join_payload()
            .returning(|_, _| Ok(vec![0x73; 64]));
        mock.expect_derive_admission_proof_key()
            .times(1)
            .return_once(move |_, _, _, _| Err(err));
        Arc::new(mock)
    }

    struct FixedProof(Vec<u8>);
    #[async_trait]
    impl ProofPort for FixedProof {
        async fn build_proof(
            &self,
            _: &SessionId,
            _: &SpaceId,
            _: [u8; 32],
            _: &ProofDerivedKey,
        ) -> anyhow::Result<SpaceAccessProofArtifact> {
            Ok(SpaceAccessProofArtifact {
                pairing_session_id: SessionId::new("fixed".to_string()),
                space_id: SpaceId::from_str("space-xyz"),
                challenge_nonce: [0x42; 32],
                proof_bytes: self.0.clone(),
            })
        }
        async fn verify_proof(
            &self,
            _: &SpaceAccessProofArtifact,
            _: [u8; 32],
        ) -> anyhow::Result<bool> {
            unimplemented!()
        }
    }

    struct FixedLocal(IdentityFingerprint);
    #[async_trait]
    impl LocalIdentityPort for FixedLocal {
        async fn create(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
            Ok(self.0.clone())
        }
        async fn ensure(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
            Ok(self.0.clone())
        }
        async fn get_current_fingerprint(
            &self,
        ) -> Result<Option<IdentityFingerprint>, LocalIdentityError> {
            Ok(Some(self.0.clone()))
        }
    }

    struct FixedDevice(DeviceId);
    impl DeviceIdentityPort for FixedDevice {
        fn current_device_id(&self) -> DeviceId {
            self.0.clone()
        }
    }

    struct StubSettings(StdMutex<Settings>);
    impl StubSettings {
        fn named(n: &str) -> Self {
            let mut s = Settings::default();
            s.general.device_name = Some(n.into());
            Self(StdMutex::new(s))
        }
        fn blank() -> Self {
            Self(StdMutex::new(Settings::default()))
        }
    }
    #[async_trait]
    impl SettingsPort for StubSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn save(&self, s: &Settings) -> anyhow::Result<()> {
            *self.0.lock().unwrap() = s.clone();
            Ok(())
        }
    }

    // ── fixtures ─────────────────────────────────────────────────────────

    const TEST_TTL: Duration = Duration::from_secs(30);

    struct TestAdmissionOwner {
        requires_session_transition: bool,
    }

    struct UnreadableHistoryAdmissionOwner;
    struct UnsupersedableJoinAdmissionOwner;

    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for UnreadableHistoryAdmissionOwner {
        async fn preflight_local_join_source(
            &self,
            preserve_unreadable_history: bool,
        ) -> Result<(), crate::space::convergence::WorkspaceConvergenceError> {
            assert!(!preserve_unreadable_history);
            Err(
                crate::space::convergence::WorkspaceConvergenceError::UnreadableHistoryRequiresConfirmation,
            )
        }

        async fn admission_decision_for_joiner(
            &self,
            _invitation_generation: u64,
            _joiner_device_id: &DeviceId,
        ) -> uc_core::membership::MembershipAdmissionDecision {
            unreachable!()
        }

        async fn synchronize_chain(
            &self,
        ) -> Result<(), crate::space::convergence::WorkspaceConvergenceError> {
            unreachable!()
        }
    }

    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for UnsupersedableJoinAdmissionOwner {
        async fn preflight_local_join_source(
            &self,
            _preserve_unreadable_history: bool,
        ) -> Result<(), crate::space::convergence::WorkspaceConvergenceError> {
            Err(
                crate::space::convergence::WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded,
            )
        }

        async fn admission_decision_for_joiner(
            &self,
            _invitation_generation: u64,
            _joiner_device_id: &DeviceId,
        ) -> uc_core::membership::MembershipAdmissionDecision {
            unreachable!()
        }

        async fn synchronize_chain(
            &self,
        ) -> Result<(), crate::space::convergence::WorkspaceConvergenceError> {
            unreachable!()
        }
    }

    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for TestAdmissionOwner {
        async fn prepare_local_join_before_network(
            &self,
            preparation: &(dyn GroupAdmissionPort + Send + Sync),
            local_device_id: &DeviceId,
            _sponsor: &[u8],
            _stable_request_binding: &[u8],
            _preserve_unreadable_history: bool,
        ) -> Result<
            crate::space::admission::adapter::DurableLocalJoinPreparation,
            crate::space::convergence::WorkspaceConvergenceError,
        > {
            let prepared_group_join = preparation
                .prepare_group_join(local_device_id)
                .await
                .map_err(|error| {
                    crate::space::convergence::WorkspaceConvergenceError::AdmissionStorage(
                        error.to_string(),
                    )
                })?;
            Ok(
                crate::space::admission::adapter::DurableLocalJoinPreparation {
                    attempt_id: [0x31; 32],
                    join_id: [0x32; 16],
                    request_message_id: [0x33; 32],
                    resume_public_key: vec![0x34; 32],
                    prepared_group_join,
                },
            )
        }

        async fn reject_local_join_before_candidate(
            &self,
            _attempt_id: [u8; 32],
            _reason: uc_core::membership::AdmissionRejectionReasonV1,
        ) -> Result<(), crate::space::convergence::WorkspaceConvergenceError> {
            Ok(())
        }

        async fn prepare_joiner_candidate(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
            _proof_signer: &(dyn GroupAdmissionPort + Send + Sync),
            _target_access: &(dyn PrepareAdmissionTargetAccessPort + Send + Sync),
            _passphrase: &Passphrase,
        ) -> Result<
            uc_core::pairing::DurableAdmissionFrame,
            crate::space::convergence::WorkspaceConvergenceError,
        > {
            Ok(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Prepared,
                Vec::new(),
            ))
        }

        async fn apply_joiner_commit(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
            _receipt_signer: &(dyn GroupAdmissionPort + Send + Sync),
        ) -> Result<
            uc_core::pairing::DurableAdmissionFrame,
            crate::space::convergence::WorkspaceConvergenceError,
        > {
            Ok(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Applied,
                Vec::new(),
            ))
        }

        async fn activate_joiner_complete(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
        ) -> Result<
            crate::space::admission::adapter::DurableJoinerCompletion,
            crate::space::convergence::WorkspaceConvergenceError,
        > {
            if self.requires_session_transition {
                Ok(crate::space::admission::adapter::DurableJoinerCompletion::SpaceTransitionRequired)
            } else {
                Ok(
                    crate::space::admission::adapter::DurableJoinerCompletion::Active(
                        durable_frame(
                            uc_core::pairing::DurableAdmissionMessageKind::CompleteAck,
                            Vec::new(),
                        ),
                    ),
                )
            }
        }

        async fn admission_decision_for_joiner(
            &self,
            _invitation_generation: u64,
            _joiner_device_id: &DeviceId,
        ) -> uc_core::membership::MembershipAdmissionDecision {
            unreachable!()
        }

        async fn synchronize_chain(
            &self,
        ) -> Result<(), crate::space::convergence::WorkspaceConvergenceError> {
            unreachable!()
        }
    }

    fn sponsor_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("BBBBBBBBBBBBBBBB").unwrap()
    }
    fn joiner_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("AAAAAAAAAAAAAAAA").unwrap()
    }

    fn durable_frame(
        kind: uc_core::pairing::DurableAdmissionMessageKind,
        payload: Vec<u8>,
    ) -> uc_core::pairing::DurableAdmissionFrame {
        uc_core::pairing::DurableAdmissionFrame {
            attempt_id: [0x31; 32],
            kind,
            message_id: [kind as u8; 32],
            predecessor_message_id: Some([0x33; 32]),
            payload,
        }
    }

    fn candidate_frame() -> uc_core::pairing::DurableAdmissionFrame {
        use uc_core::membership::{
            AdmissionChangeFacts, MembershipCredential, MembershipEventV2, MembershipOperationV2,
            ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
        };

        let sponsor_device = DeviceId::new("sponsor-device");
        let sponsor_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
        let sponsor_member = sponsor_credential.member_instance_id(&sponsor_device);
        let event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            "space-xyz".to_owned(),
            None,
            0,
            [0x41; 16],
            sponsor_member,
            sponsor_credential.credential_id,
            sponsor_credential.signature_algorithm_version,
            MembershipOperationV2::RemoveDevice {
                member: uc_core::membership::MemberInstanceId::from_bytes([0x42; 32]),
            },
            [0x43; 32],
            [0x44; 32],
            Vec::new(),
            None,
            vec![0x45; 64],
        );
        let candidate = crate::space::convergence::admission::DurableAdmissionCandidateV1 {
            lineage_id: "space-xyz".to_owned(),
            base_history_position: Vec::new(),
            candidate_event: postcard::to_stdvec(&event).unwrap(),
            candidate_event_id: *event.event_id().as_bytes(),
            candidate_key_package: vec![1],
            resume_public_key: vec![2],
            target_members_digest: [0x43; 32],
            security_commitment: vec![3],
            security_commit: vec![4],
            security_welcome: vec![5],
            target_protection_group_id: "target-group".to_owned(),
            target_key_catalog: vec![6],
            target_relationships: vec![AdmissionChangeFacts {
                member_instance: sponsor_member,
                device_id: sponsor_device,
                device_name: "sponsor's laptop".to_owned(),
                identity_fingerprint: sponsor_fp(),
                transport_public_key: vec![7],
                transport_address_blob: Vec::new(),
                identity_signature: vec![8],
            }],
            existing_member_deliveries: Vec::new(),
            staged_security_state: vec![9],
            identity_binding: vec![10],
        };
        let payload =
            crate::space::convergence::admission::DurableAdmissionCandidatePayloadV1::new(
                Vec::new(),
                candidate,
            )
            .encode()
            .unwrap();
        durable_frame(
            uc_core::pairing::DurableAdmissionMessageKind::Candidate,
            payload,
        )
    }
    fn admission_offer() -> SponsorAdmissionOffer {
        SponsorAdmissionOffer {
            space_id: SpaceId::from_str("space-xyz"),
            kdf_parameters_blob: vec![0xAA; 16],
            challenge: vec![0x42; 32],
            pairing_session_id: PairingSessionId::new("session-1"),
        }
    }
    struct Bundle {
        session: Arc<ScriptedSession>,
        space_access: Arc<MockSpaceAccess>,
        settings: Arc<StubSettings>,
    }

    impl Bundle {
        fn happy() -> Self {
            Self {
                session: Arc::new(ScriptedSession::with_dial_ok("joiner-session")),
                space_access: space_access(),
                settings: Arc::new(StubSettings::named("joiner-laptop")),
            }
        }
        fn with_dial_err(err: DialError) -> Self {
            let mut b = Self::happy();
            b.session = Arc::new(ScriptedSession::with_dial_err(err));
            b
        }

        fn build(&self) -> Arc<JoinerHandshakeCoordinator> {
            self.build_with_transition(false)
        }

        fn build_with_transition(
            &self,
            requires_session_transition: bool,
        ) -> Arc<JoinerHandshakeCoordinator> {
            JoinerHandshakeCoordinator::new(
                self.session.clone(),
                self.space_access.clone(),
                self.space_access.clone(),
                self.space_access.clone(),
                Arc::new(FixedProof(vec![0xFE; 32])),
                Arc::new(FixedLocal(joiner_fp())),
                Arc::new(FixedDevice(DeviceId::new("joiner-device"))),
                self.settings.clone(),
                Arc::new(TestAdmissionOwner {
                    requires_session_transition,
                }),
                TEST_TTL,
            )
        }
    }

    fn code(s: &str) -> InvitationCode {
        InvitationCode::new(s)
    }
    fn passphrase() -> Passphrase {
        Passphrase::new("hunter22hunter22")
    }

    #[tokio::test]
    async fn unreadable_source_history_is_rejected_before_dial_without_confirmation() {
        let bundle = Bundle::happy();
        let coordinator = JoinerHandshakeCoordinator::new(
            bundle.session.clone(),
            bundle.space_access.clone(),
            bundle.space_access.clone(),
            bundle.space_access.clone(),
            Arc::new(FixedProof(vec![0xFE; 32])),
            Arc::new(FixedLocal(joiner_fp())),
            Arc::new(FixedDevice(DeviceId::new("joiner-device"))),
            bundle.settings.clone(),
            Arc::new(UnreadableHistoryAdmissionOwner),
            TEST_TTL,
        );

        let result = coordinator
            .handshake_with_history_policy(&code("CODE-1"), &passphrase(), false)
            .await;

        assert!(matches!(
            result,
            Err(RedeemPairingInvitationError::UnreadableHistoryRequiresConfirmation)
        ));
        assert_eq!(bundle.session.dial_calls(), 0);
        assert!(bundle.session.sent().is_empty());
    }

    #[tokio::test]
    async fn unsupersedable_join_is_rejected_before_dial() {
        let bundle = Bundle::happy();
        let coordinator = JoinerHandshakeCoordinator::new(
            bundle.session.clone(),
            bundle.space_access.clone(),
            bundle.space_access.clone(),
            bundle.space_access.clone(),
            Arc::new(FixedProof(vec![0xFE; 32])),
            Arc::new(FixedLocal(joiner_fp())),
            Arc::new(FixedDevice(DeviceId::new("joiner-device"))),
            bundle.settings.clone(),
            Arc::new(UnsupersedableJoinAdmissionOwner),
            TEST_TTL,
        );

        let result = coordinator
            .handshake_with_history_policy(&code("CODE-1"), &passphrase(), false)
            .await;

        assert!(matches!(
            result,
            Err(RedeemPairingInvitationError::PreviousJoinCannotBeSuperseded)
        ));
        assert_eq!(bundle.session.dial_calls(), 0);
        assert!(bundle.session.sent().is_empty());
    }

    // ── happy path ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn happy_path_outcome_and_wire_sequence() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::DurableAdmission(
                candidate_frame(),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::DurableAdmission(
                durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::Commit,
                    Vec::new(),
                ),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::DurableAdmission(
                durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::Complete,
                    Vec::new(),
                ),
            )));
        let coord = b.build();

        let out = coord
            .handshake(&code("CODE-1"), &passphrase())
            .await
            .unwrap();

        assert_eq!(out.outcome().sponsor_device_id.as_str(), "sponsor-device");
        assert_eq!(out.outcome().sponsor_device_name, "sponsor's laptop");
        assert_eq!(out.outcome().sponsor_identity_fingerprint, sponsor_fp());
        assert_eq!(out.outcome().space_id.inner(), "space-xyz");
        assert_eq!(out.outcome().self_device_id.as_str(), "joiner-device");
        assert_eq!(out.outcome().self_identity_fingerprint, joiner_fp());
        assert!(!out.requires_session_transition());

        let sent = b.session.sent();
        assert_eq!(sent.len(), 5);
        match &sent[0].1 {
            PairingSessionMessage::Request(r) => {
                assert_eq!(r.attempt_id, [0x31; 32]);
                assert_eq!(r.join_id, [0x32; 16]);
                assert_eq!(r.request_message_id, [0x33; 32]);
                assert_eq!(r.invitation_code.as_str(), "CODE-1");
                assert_eq!(r.device_id.as_str(), "joiner-device");
                assert_eq!(r.device_name, "joiner-laptop");
                assert_eq!(r.identity_fingerprint, joiner_fp());
                assert_eq!(
                    r.membership_credential.member_instance_id(&r.device_id),
                    r.member_instance
                );
                assert_eq!(r.admission.member_instance, r.member_instance);
                assert_eq!(r.admission.device_id, r.device_id);
                assert_eq!(r.admission.transport_public_key, vec![0x71; 32]);
                assert_eq!(r.admission.identity_signature, vec![0x73; 64]);
                assert_eq!(r.resume_public_key, vec![0x34; 32]);
                r.validate_durable_identity().unwrap();
            }
            other => panic!("expected Request, got {other:?}"),
        }
        assert!(matches!(
            sent[1].1,
            PairingSessionMessage::ChallengeResponse(JoinerChallengeResponse { .. })
        ));
        assert!(matches!(
            sent[2].1,
            PairingSessionMessage::DurableAdmission(ref frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::Prepared
        ));
        assert!(matches!(
            sent[3].1,
            PairingSessionMessage::DurableAdmission(ref frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::Applied
        ));
        assert!(matches!(
            sent[4].1,
            PairingSessionMessage::DurableAdmission(ref frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::CompleteAck
        ));
        assert_eq!(b.session.closed().len(), 0, "sponsor closes the session");
    }

    #[tokio::test]
    async fn cross_space_completion_waits_for_session_transition_before_acknowledging() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::DurableAdmission(
                candidate_frame(),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::DurableAdmission(
                durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::Commit,
                    Vec::new(),
                ),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::DurableAdmission(
                durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::Complete,
                    Vec::new(),
                ),
            )));

        let pending = b
            .build_with_transition(true)
            .handshake(&code("CODE-1"), &passphrase())
            .await
            .unwrap();

        assert!(pending.requires_session_transition());
        assert_eq!(b.session.sent().len(), 4);
        assert!(b.session.sent().iter().all(|(_, message)| !matches!(
            message,
            PairingSessionMessage::DurableAdmission(frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::CompleteAck
        )));
    }

    #[test]
    fn stable_join_request_binding_excludes_display_and_transport_fields() {
        let device = DeviceId::new("joiner-device");
        let fingerprint = joiner_fp();

        let before = stable_join_request_binding(&device, &fingerprint);
        let after_device_rename_and_address_change =
            stable_join_request_binding(&device, &fingerprint);

        assert_eq!(before, after_device_rename_and_address_change);
        assert!(!before
            .windows("old laptop".len())
            .any(|part| part == b"old laptop"));
        assert!(!before
            .windows("new laptop".len())
            .any(|part| part == b"new laptop"));
    }

    // ── dial errors ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn dial_invitation_not_found_maps_and_no_wire_activity() {
        let b = Bundle::with_dial_err(DialError::InvitationNotFound);
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::InvitationNotFound
        ));
        assert!(b.session.sent().is_empty());
        assert!(b.session.closed().is_empty());
    }

    #[tokio::test]
    async fn dial_invitation_expired_maps() {
        let b = Bundle::with_dial_err(DialError::InvitationExpired);
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::InvitationExpired
        ));
    }

    #[tokio::test]
    async fn dial_sponsor_unreachable_maps() {
        let b = Bundle::with_dial_err(DialError::SponsorUnreachable);
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::SponsorUnreachable
        ));
    }

    #[tokio::test]
    async fn dial_service_unavailable_maps() {
        let b = Bundle::with_dial_err(DialError::ServiceUnavailable);
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::ServiceUnavailable
        ));
    }

    #[tokio::test]
    async fn legacy_sponsor_requires_an_upgrade_before_any_wire_or_local_state_work() {
        let b = Bundle::with_dial_err(DialError::SponsorUpgradeRequired);

        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            RedeemPairingInvitationError::SponsorUpgradeRequired
        ));
        assert!(b.session.sent().is_empty());
        assert!(b.session.closed().is_empty());
    }

    // ── sponsor rejects ──────────────────────────────────────────────────

    #[tokio::test]
    async fn sponsor_reject_invitation_mismatch_after_request() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::Reject(
                PairingReject {
                    reason: PairingRejectReason::InvitationMismatch,
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::SponsorRejectedInvitation
        ));
        assert_eq!(b.session.sent().len(), 1);
        assert_eq!(b.session.closed().len(), 1);
    }

    #[tokio::test]
    async fn sponsor_reject_passphrase_mismatch_after_challenge() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::Reject(
                PairingReject {
                    reason: PairingRejectReason::PassphraseMismatch,
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::PassphraseMismatch
        ));
        assert_eq!(b.session.sent().len(), 2);
    }

    #[tokio::test]
    async fn sponsor_reject_timeout_maps_to_sponsor_timed_out() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::Reject(
                PairingReject {
                    reason: PairingRejectReason::Timeout,
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(err, RedeemPairingInvitationError::SponsorTimedOut));
    }

    #[tokio::test]
    async fn sponsor_reject_user_rejected_maps_to_sponsor_declined() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::Reject(
                PairingReject {
                    reason: PairingRejectReason::UserRejected,
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(err, RedeemPairingInvitationError::SponsorDeclined));
    }

    #[tokio::test]
    async fn sponsor_reject_internal_carries_message() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::Reject(
                PairingReject {
                    reason: PairingRejectReason::Internal("oops".into()),
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        match err {
            RedeemPairingInvitationError::SponsorInternal(m) => assert_eq!(m, "oops"),
            other => panic!("expected SponsorInternal, got {other:?}"),
        }
    }

    // ── own TTL ──────────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn ttl_fires_on_first_recv_when_sponsor_silent() {
        let b = Bundle::happy();
        b.session.push_recv(RecvStep::Hang);
        let coord = b.build();
        let handle = tokio::spawn(async move { coord.handshake(&code("X"), &passphrase()).await });
        tokio::time::sleep(TEST_TTL + ChronoDuration::seconds(1).to_std().unwrap()).await;
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, RedeemPairingInvitationError::Timeout));
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_fires_on_second_recv_when_sponsor_silent_after_offer() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        b.session.push_recv(RecvStep::Hang);
        let sent_probe = b.session.clone();
        let closed_probe = b.session.clone();
        let coord = b.build();
        let handle = tokio::spawn(async move { coord.handshake(&code("X"), &passphrase()).await });
        tokio::time::sleep(TEST_TTL + ChronoDuration::seconds(1).to_std().unwrap()).await;
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, RedeemPairingInvitationError::Timeout));
        assert_eq!(sent_probe.sent().len(), 2);
        assert_eq!(closed_probe.closed().len(), 1);
    }

    // ── local derive failures ────────────────────────────────────────────

    #[tokio::test]
    async fn local_wrong_passphrase_maps_to_passphrase_mismatch() {
        let mut b = Bundle::happy();
        b.space_access = space_access_with_derivation_error(SpaceAccessError::WrongPassphrase);
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::PassphraseMismatch
        ));
        // Only Request went out — ChallengeResponse never sent.
        assert_eq!(b.session.sent().len(), 1);
    }

    #[tokio::test]
    async fn local_corrupted_keyslot_maps_to_corrupted() {
        let mut b = Bundle::happy();
        b.space_access = space_access_with_derivation_error(SpaceAccessError::CorruptedKeyMaterial);
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::CorruptedKeyMaterial
        ));
    }

    // ── connection / protocol errors ─────────────────────────────────────

    #[tokio::test]
    async fn connection_closed_before_offer_maps_to_connection_lost() {
        let b = Bundle::happy();
        b.session.push_recv(RecvStep::CleanClose);
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(err, RedeemPairingInvitationError::ConnectionLost));
    }

    #[tokio::test]
    async fn session_error_during_recv_maps_to_connection_lost() {
        let b = Bundle::happy();
        b.session.push_recv(RecvStep::Err(SessionError::Closed));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(err, RedeemPairingInvitationError::ConnectionLost));
    }

    #[tokio::test]
    async fn unexpected_first_frame_surfaces_internal() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::ChallengeResponse(
                JoinerChallengeResponse {
                    encrypted_challenge: vec![0x42; 32],
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        match err {
            RedeemPairingInvitationError::Internal(m) => {
                assert!(m.contains("expected AdmissionOffer"), "msg = {m}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unexpected_second_frame_surfaces_internal() {
        let b = Bundle::happy();
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::AdmissionOffer(
                admission_offer(),
            )));
        let device_id = DeviceId::new("x");
        let credential = uc_core::membership::MembershipCredential::new(1, vec![4; 32]);
        let member_instance = credential.member_instance_id(&device_id);
        b.session
            .push_recv(RecvStep::Msg(PairingSessionMessage::Request(
                JoinerRequest {
                    attempt_id: [1; 32],
                    join_id: [2; 16],
                    request_message_id: [3; 32],
                    invitation_code: InvitationCode::new("X"),
                    device_id: device_id.clone(),
                    device_name: "x".into(),
                    identity_fingerprint: joiner_fp(),
                    nonce: vec![],
                    transport_address_blob: vec![],
                    security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
                    key_package: vec![1, 2, 3],
                    member_instance,
                    membership_credential: credential,
                    resume_public_key: vec![5; 32],
                    admission: AdmissionChangeFacts {
                        member_instance,
                        device_id,
                        device_name: "x".into(),
                        identity_fingerprint: joiner_fp(),
                        transport_public_key: vec![6; 32],
                        transport_address_blob: vec![],
                        identity_signature: vec![7; 64],
                    },
                },
            )));
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        match err {
            RedeemPairingInvitationError::Internal(m) => {
                assert!(m.contains("expected Candidate"), "msg = {m}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    // ── local facts ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn device_name_missing_short_circuits_before_any_wire_send() {
        let mut b = Bundle::happy();
        b.settings = Arc::new(StubSettings::blank());
        let err = b
            .build()
            .handshake(&code("X"), &passphrase())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::DeviceNameRequired
        ));
        assert!(b.session.sent().is_empty());
        // Session was dialled so close fires on the error path.
        assert_eq!(b.session.closed().len(), 1);
    }
}
