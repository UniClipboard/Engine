//! Sponsor-side handshake wire adapter.
//!
//! Owns the transport-level conversation with one joiner for the duration
//! of a pairing session: sends `AdmissionOffer`, verifies the joiner's
//! `ChallengeResponse`, and emits `Confirm` / `Reject` on the wire.
//! Persistence (SpaceMember / TrustedPeer) is intentionally **not** done
//! here — the outer [`super::orchestrator::PairingInboundOrchestrator`]
//! composes this coordinator with `AdmitMemberUseCase` and
//! `TrustPeerUseCase`.
//!
//! ## Why no FSM on this side
//!
//! Sponsor path is linear (`begin → verify → confirm | reject → close`)
//! with the only branch sitting on the `verify_proof` verdict. Running
//! it through a generic access state machine would give us enum
//! ceremony without extra correctness guarantees, and the candidate
//! FSM's action order for the verified branch (`SendResult` →
//! `PersistSponsorAccess`) was **inverted** from the ordering Slice 1
//! wants (persist must happen before Confirm so an admit failure
//! cannot strand the joiner with a committed Confirm
//! they can't reach). The joiner side (P7h) has more states and genuine
//! user-input branches; it will reuse the FSM.
//!
//! ## TTL 与内部 tokio::spawn
//!
//! `begin` 成功后会 spawn 一个 TTL 看门狗 task，超时则由 coordinator
//! 自己发 `Reject(Timeout)` + close + drop ctx（P7g）。`confirm` /
//! `reject` / `handle_session_closed` 任一先触发都会 abort 这个 task。
//! 这里没走 `TimerPort`：后者没有 fire-back 回调，对 handshake 这种
//! 需要"超时即主动 Reject"的语义用不上；在 coordinator 内直接 spawn
//! 符合 `uc-application/AGENTS.md` §15.3（运行时细节收敛在 orchestrator
//! 内部）。
//!
//! 为了让 spawn 的闭包能回调 coordinator 自己，用 `Arc::new_cyclic`
//! 持一个 `Weak<Self>`；`new` 因此返回 `Arc<Self>` 而不是 `Self`。
//!
//! The coordinator is `pub(crate)` — external callers reach pairing
//! exclusively through the orchestrator's `handle_event` entry.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::Mutex;
use tokio::task::AbortHandle;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

use uc_core::ids::{SessionId, SpaceId};
use uc_core::pairing::session_message::{
    JoinerChallengeResponse, JoinerRequest, PairingReject, PairingRejectReason,
    PairingSecurityCapability, PairingSessionMessage, SponsorAdmissionOffer,
};
use uc_core::ports::pairing::{PairingSessionId, PairingSessionPort};
use uc_core::ports::space::{PrepareAdmissionOfferPort, ProofPort};
use uc_core::space_access::domain::{ProofDerivedKey, SpaceAccessProofArtifact};

use crate::space::current_space::CurrentSpaceIdentityPort;
use crate::space::workspace_membership::membership::group_update_delivery::GroupUpdateDeliveryPort;

/// Facts about the verified joiner, handed to the orchestrator so it can
/// drive admit + trust use cases without re-parsing the `JoinerRequest`.
#[derive(Debug, Clone)]
pub(crate) struct JoinerFacts {
    pub request: JoinerRequest,
}

/// Outcome of the joiner's `ChallengeResponse`.
#[derive(Debug)]
pub(crate) enum Verdict {
    Verified(JoinerFacts),
    Rejected,
}

/// Per-session data parked between `AdmissionOffer` (sent) and
/// `ChallengeResponse` (received). Dropped on any terminal outcome
/// (Confirm, Reject, peer-initiated Close, TTL timeout).
struct SessionCtx {
    space_id: SpaceId,
    /// 32-byte nonce we handed the joiner; feeds `verify_proof`.
    challenge_nonce: [u8; 32],
    /// HMAC binding input — same string both sides independently derive
    /// from `PairingSessionId`, so replay across sessions fails.
    core_session_id: SessionId,
    verification_key: Arc<ProofDerivedKey>,
    joiner: JoinerFacts,
    /// TTL 看门狗 task 的 abort handle；在 ctx 被正常终止
    /// (confirm / reject / session closed) 时 abort，让 task 不再 fire。
    /// `None` 仅出现在 timer spawn 前的极短窗口或 race（ctx 已被抢先
    /// 清掉），此时 `begin` 兜底立即 abort 自己 spawn 的 handle。
    timer_abort: Option<AbortHandle>,
}

pub(crate) struct SponsorHandshakeCoordinator {
    pairing_session: Arc<dyn PairingSessionPort>,
    space_access: Arc<dyn PrepareAdmissionOfferPort>,
    group_update_delivery: Arc<dyn GroupUpdateDeliveryPort>,
    proof_port: Arc<dyn ProofPort>,
    /// Source of the stable `SpaceId` persisted at A1 time. The earlier
    /// design minted a fresh UUID per handshake, which caused the joiner
    /// to adopt an id unrelated to the sponsor's original space — this
    /// port fixes that by giving `begin` access to the canonical value.
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    sessions: Mutex<HashMap<PairingSessionId, SessionCtx>>,
    /// handshake TTL (max wait between begin and confirm/reject).
    handshake_ttl: Duration,
    /// Self-referential Weak so the TTL watchdog task can call back
    /// without forming an Arc cycle.
    self_weak: Weak<Self>,
}

impl SponsorHandshakeCoordinator {
    pub(crate) fn new(
        pairing_session: Arc<dyn PairingSessionPort>,
        space_access: Arc<dyn PrepareAdmissionOfferPort>,
        group_update_delivery: Arc<dyn GroupUpdateDeliveryPort>,
        proof_port: Arc<dyn ProofPort>,
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        handshake_ttl: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            pairing_session,
            space_access,
            group_update_delivery,
            proof_port,
            current_space_identity,
            sessions: Mutex::new(HashMap::new()),
            handshake_ttl,
            self_weak: weak.clone(),
        })
    }

    /// Step 1: prepare + send `AdmissionOffer`, park per-session state.
    ///
    /// On success the session is ready to receive `ChallengeResponse`.
    /// On failure the coordinator itself sends `Reject(Internal)` and
    /// closes the transport — the orchestrator has nothing to do but
    /// observe the `Err`.
    pub(crate) async fn begin(
        &self,
        session: &PairingSessionId,
        request: JoinerRequest,
    ) -> Result<(), ()> {
        info!(
            session = %session,
            joiner_device_id = %request.device_id.as_str(),
            transport_address_blob_len = request.transport_address_blob.len(),
            "sponsor pairing begin; preparing AdmissionOffer"
        );
        if request.security_capability != PairingSecurityCapability::ReliableGroupEpochV1
            || request.key_package.is_empty()
        {
            self.send_reject_and_close(
                session,
                PairingRejectReason::Internal("secure group admission capability required".into()),
            )
            .await;
            return Err(());
        }
        let probe_space_id = match self.current_space_identity.current_space_id().await {
            Ok(Some(space_id)) => space_id,
            Ok(None) => {
                warn!(session = %session, "current Space is absent; rejecting pairing");
                self.send_reject_and_close(
                    session,
                    PairingRejectReason::Internal("current Space is unavailable".into()),
                )
                .await;
                return Err(());
            }
            Err(err) => {
                warn!(
                    session = %session,
                    error = %err,
                    "current Space lookup failed; rejecting pairing"
                );
                self.send_reject_and_close(
                    session,
                    PairingRejectReason::Internal("setup status unavailable".into()),
                )
                .await;
                return Err(());
            }
        };
        let core_session_id = SessionId::new(session.as_str().to_string());

        let offer = match self
            .space_access
            .prepare_admission_offer(&probe_space_id, &request.invitation_code, &core_session_id)
            .await
        {
            Ok(o) => o,
            Err(err) => {
                warn!(
                    session = %session,
                    error = %err,
                    "prepare_join_offer failed; rejecting inbound pairing"
                );
                self.send_reject_and_close(
                    session,
                    PairingRejectReason::Internal(format!("prepare_admission_offer: {err}")),
                )
                .await;
                return Err(());
            }
        };

        let ctx = SessionCtx {
            space_id: offer.offer.space_id.clone(),
            challenge_nonce: offer.offer.challenge_nonce,
            core_session_id,
            verification_key: Arc::new(offer.verification_key),
            joiner: JoinerFacts { request },
            timer_abort: None,
        };
        // Park state *before* sending so a racing ChallengeResponse
        // always finds a home (iroh send is faster than a wire round
        // trip, but we don't rely on that).
        self.sessions.lock().await.insert(session.clone(), ctx);

        let keyslot = PairingSessionMessage::AdmissionOffer(SponsorAdmissionOffer {
            space_id: offer.offer.space_id,
            kdf_parameters_blob: offer.offer.kdf_parameters_blob,
            challenge: offer.offer.challenge_nonce.to_vec(),
            pairing_session_id: session.clone(),
        });
        if let Err(err) = self.pairing_session.send(session, keyslot).await {
            warn!(
                session = %session,
                error = %err,
                "AdmissionOffer send failed; dropping ctx and closing"
            );
            self.sessions.lock().await.remove(session);
            self.pairing_session
                .close(session, Some("AdmissionOffer send failed".into()))
                .await;
            return Err(());
        }

        self.arm_timeout(session).await;

        info!(
            session = %session,
            ttl_ms = %self.handshake_ttl.as_millis(),
            "AdmissionOffer sent; awaiting ChallengeResponse"
        );
        Ok(())
    }

    /// Spawn the TTL watchdog and stash its `AbortHandle` in the parked
    /// ctx. If the ctx was already gone (e.g. a racing `reject` cleared
    /// it between the `send` return and this call), the handle is
    /// aborted immediately so the task never fires. Called only from
    /// `begin` after a successful `AdmissionOffer` send.
    async fn arm_timeout(&self, session: &PairingSessionId) {
        let ttl = self.handshake_ttl;
        let weak = self.self_weak.clone();
        let session_for_task = session.clone();
        let join = tokio::spawn(async move {
            sleep(ttl).await;
            if let Some(this) = weak.upgrade() {
                this.fire_timeout(&session_for_task).await;
            }
        });
        let handle = join.abort_handle();
        let mut map = self.sessions.lock().await;
        match map.get_mut(session) {
            Some(ctx) => ctx.timer_abort = Some(handle),
            None => {
                // ctx vanished in the window between send-success and
                // arming (only possible via a concurrent terminal op
                // racing us). The task is now orphaned — kill it so it
                // can't fire a ghost Reject after the slot is reused.
                handle.abort();
            }
        }
    }

    /// Called by the spawned watchdog when the TTL expires. Idempotent:
    /// if confirm/reject/close already cleared the ctx, this is a no-op
    /// (the `AbortHandle` cancel usually wins; this is the loss-case
    /// guard).
    async fn fire_timeout(&self, session: &PairingSessionId) {
        let removed = self.sessions.lock().await.remove(session).is_some();
        if !removed {
            return;
        }
        info!(
            session = %session,
            ttl_ms = %self.handshake_ttl.as_millis(),
            "handshake TTL expired; rejecting joiner with Timeout"
        );
        self.send_reject_and_close(session, PairingRejectReason::Timeout)
            .await;
    }

    /// Step 2: run `verify_proof` against the parked nonce and return
    /// the outcome. Does **not** touch the wire — the orchestrator
    /// decides next move via `confirm` or `reject`. State stays parked
    /// until one of those is called (or `handle_session_closed`).
    ///
    /// `None` means there was no live ctx under `session` (e.g. the
    /// joiner sent `ChallengeResponse` without a preceding `Request`,
    /// or we already finalised the session). Caller should drop it.
    pub(crate) async fn verify_challenge(
        &self,
        session: &PairingSessionId,
        response: JoinerChallengeResponse,
    ) -> Option<Verdict> {
        // Peek, not remove — state must stay for a follow-up confirm /
        // reject call. `handle_session_closed` is the only cleaner if
        // neither fires.
        let (artifact, facts, verification_key) = {
            let map = self.sessions.lock().await;
            let ctx = map.get(session)?;
            let artifact = SpaceAccessProofArtifact {
                pairing_session_id: ctx.core_session_id.clone(),
                space_id: ctx.space_id.clone(),
                challenge_nonce: ctx.challenge_nonce,
                proof_bytes: response.encrypted_challenge,
            };
            (
                artifact,
                ctx.joiner.clone(),
                Arc::clone(&ctx.verification_key),
            )
        };

        let verified = match self
            .proof_port
            .verify_proof_with_key(&artifact, artifact.challenge_nonce, &verification_key)
            .await
        {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    session = %session,
                    error = %err,
                    "proof verification errored; treating as invalid"
                );
                false
            }
        };

        Some(if verified {
            info!(
                session = %session,
                joiner_device_id = %facts.request.device_id.as_str(),
                "joiner proof verified"
            );
            Verdict::Verified(facts)
        } else {
            info!(
                session = %session,
                joiner_device_id = %facts.request.device_id.as_str(),
                "joiner proof rejected"
            );
            Verdict::Rejected
        })
    }

    pub(crate) async fn send_durable_candidate(
        &self,
        session: &PairingSessionId,
        candidate: uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), String> {
        let ctx = self
            .sessions
            .lock()
            .await
            .remove(session)
            .ok_or_else(|| "candidate send called without parked ctx".to_owned())?;
        abort_timer(&ctx);
        self.pairing_session
            .send(session, PairingSessionMessage::DurableAdmission(candidate))
            .await
            .map_err(|error| format!("send Candidate: {error}"))?;
        info!(session = %session, "durable admission Candidate sent");
        Ok(())
    }

    pub(crate) async fn send_durable_frame(
        &self,
        session: &PairingSessionId,
        frame: uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), String> {
        self.pairing_session
            .send(session, PairingSessionMessage::DurableAdmission(frame))
            .await
            .map_err(|error| format!("send durable admission frame: {error}"))
    }

    /// Step 4c: close the session after the admission change was committed
    /// and confirmed. No member state is saved here; the workspace owner
    /// already did that at its commit point.
    pub(crate) async fn complete(&self, session: &PairingSessionId) {
        if let Err(error) = self
            .group_update_delivery
            .deliver_pending(chrono::Utc::now().timestamp_millis())
            .await
        {
            warn!(error = %error, "pending group updates could not be delivered after joiner readiness");
        }
        self.pairing_session
            .close(session, Some("joiner ready".into()))
            .await;
    }

    /// Step 3b: build + send `Reject(reason)`, close session, drop ctx.
    /// Idempotent on missing ctx — the defensive remove at the top
    /// means calling this after the orchestrator already cleared state
    /// via some other path is a no-op.
    pub(crate) async fn reject(&self, session: &PairingSessionId, reason: PairingRejectReason) {
        if let Some(ctx) = self.sessions.lock().await.remove(session) {
            abort_timer(&ctx);
        }
        self.send_reject_and_close(session, reason).await;
    }

    /// Release any parked state for a session the transport reports as
    /// closed (peer hung up, underlying connection error, etc.).
    pub(crate) async fn handle_session_closed(
        &self,
        session: &PairingSessionId,
        reason: Option<&str>,
    ) {
        if let Some(ctx) = self.sessions.lock().await.remove(session) {
            abort_timer(&ctx);
            debug!(
                session = %session,
                reason = ?reason,
                "session closed with parked handshake ctx; released"
            );
        }
    }

    #[cfg(test)]
    pub(crate) async fn parked_sessions(&self) -> usize {
        self.sessions.lock().await.len()
    }

    async fn send_reject_and_close(&self, session: &PairingSessionId, reason: PairingRejectReason) {
        let reject = PairingSessionMessage::Reject(PairingReject {
            reason: reason.clone(),
        });
        if let Err(err) = self.pairing_session.send(session, reject).await {
            warn!(
                session = %session,
                error = %err,
                "failed to deliver Reject; closing anyway"
            );
        }
        self.pairing_session
            .close(session, Some(format!("reject: {reason:?}")))
            .await;
    }
}

/// Abort the TTL watchdog attached to a just-removed ctx. Free fn (not a
/// method) because by the time this runs the ctx has already been
/// extracted from the HashMap and no longer belongs to the coordinator.
fn abort_timer(ctx: &SessionCtx) {
    if let Some(h) = &ctx.timer_abort {
        h.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;

    use uc_core::ids::DeviceId;
    use uc_core::membership::AdmissionChangeFacts;
    use uc_core::pairing::invitation::InvitationCode;
    use uc_core::ports::pairing::{DialError, DialOutcome, SessionError};
    use uc_core::ports::space::{PrepareAdmissionOfferPort, SpaceAccessError};
    use uc_core::ports::SettingsPort;
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;
    use uc_core::space_access::domain::{PreparedAdmissionOffer, ProofDerivedKey};

    // ── fakes ────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct RecordingSessionPort {
        sent: StdMutex<Vec<(PairingSessionId, PairingSessionMessage)>>,
        closed: StdMutex<Vec<(PairingSessionId, Option<String>)>>,
        fail_send: StdMutex<bool>,
    }

    impl RecordingSessionPort {
        fn sent(&self) -> Vec<(PairingSessionId, PairingSessionMessage)> {
            self.sent.lock().unwrap().clone()
        }
        fn closed(&self) -> Vec<(PairingSessionId, Option<String>)> {
            self.closed.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PairingSessionPort for RecordingSessionPort {
        async fn dial_by_invitation(
            &self,
            _code: &InvitationCode,
        ) -> Result<DialOutcome, DialError> {
            unimplemented!()
        }
        async fn send(
            &self,
            session: &PairingSessionId,
            message: PairingSessionMessage,
        ) -> Result<(), SessionError> {
            if *self.fail_send.lock().unwrap() {
                return Err(SessionError::Closed);
            }
            self.sent.lock().unwrap().push((session.clone(), message));
            Ok(())
        }
        async fn recv_next(
            &self,
            _session: &PairingSessionId,
        ) -> Result<Option<PairingSessionMessage>, SessionError> {
            unimplemented!()
        }
        async fn close(&self, session: &PairingSessionId, reason: Option<String>) {
            self.closed.lock().unwrap().push((session.clone(), reason));
        }
    }

    mockall::mock! {
        SpaceAccess {}

        #[async_trait]
        impl PrepareAdmissionOfferPort for SpaceAccess {
            async fn prepare_admission_offer(
                &self,
                space_id: &SpaceId,
                invitation_code: &InvitationCode,
                pairing_session_id: &SessionId,
            ) -> Result<PreparedAdmissionOffer, SpaceAccessError>;
        }
    }

    mockall::mock! {
        GroupUpdateDelivery {}

        #[async_trait]
        impl GroupUpdateDeliveryPort for GroupUpdateDelivery {
            async fn deliver_pending(
                &self,
                now_ms: i64,
            ) -> Result<usize, uc_core::membership::KeyEpochError>;
        }
    }

    fn noop_delivery() -> Arc<MockGroupUpdateDelivery> {
        let mut delivery = MockGroupUpdateDelivery::new();
        delivery.expect_deliver_pending().returning(|_| Ok(0));
        Arc::new(delivery)
    }

    fn space_access() -> Arc<MockSpaceAccess> {
        let mut access = MockSpaceAccess::new();
        access
            .expect_prepare_admission_offer()
            .times(0..=1)
            .returning(|_, _, _| {
                Ok(PreparedAdmissionOffer {
                    offer: uc_core::space_access::AdmissionOffer {
                        space_id: SpaceId::from_str("space-xyz"),
                        kdf_parameters_blob: vec![0xAA; 32],
                        challenge_nonce: [0x42; 32],
                    },
                    verification_key: ProofDerivedKey::from_bytes([0x55; 32]),
                })
            });
        Arc::new(access)
    }

    fn failing_space_access() -> Arc<MockSpaceAccess> {
        let mut access = MockSpaceAccess::new();
        access
            .expect_prepare_admission_offer()
            .times(1)
            .returning(|_, _, _| Err(SpaceAccessError::Internal("boom".into())));
        Arc::new(access)
    }

    struct ScriptedProof(StdMutex<Vec<anyhow::Result<bool>>>);
    #[async_trait]
    impl ProofPort for ScriptedProof {
        async fn build_proof(
            &self,
            _: &SessionId,
            _: &SpaceId,
            _: [u8; 32],
            _: &ProofDerivedKey,
        ) -> anyhow::Result<SpaceAccessProofArtifact> {
            unimplemented!()
        }
        async fn verify_proof(
            &self,
            _: &SpaceAccessProofArtifact,
            _: [u8; 32],
        ) -> anyhow::Result<bool> {
            let mut q = self.0.lock().unwrap();
            if q.is_empty() {
                return Ok(false);
            }
            q.remove(0)
        }
    }

    struct StubSettings(StdMutex<Settings>);
    impl StubSettings {
        fn named(n: &str) -> Self {
            let mut s = Settings::default();
            s.general.device_name = Some(n.into());
            Self(StdMutex::new(s))
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

    // ── helpers ──────────────────────────────────────────────────────────

    fn joiner_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("AAAAAAAAAAAAAAAA").unwrap()
    }
    fn joiner_request() -> JoinerRequest {
        let device_id = DeviceId::new("joiner-device");
        let credential = uc_core::membership::MembershipCredential::new(1, vec![4; 32]);
        let member_instance = credential.member_instance_id(&device_id);
        JoinerRequest {
            attempt_id: [1; 32],
            join_id: [2; 16],
            request_message_id: [3; 32],
            invitation_code: InvitationCode::new("C"),
            device_id: device_id.clone(),
            device_name: "joiner's laptop".into(),
            identity_fingerprint: joiner_fp(),
            nonce: vec![1, 2, 3, 4],
            transport_address_blob: vec![],
            security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
            key_package: vec![1, 2, 3],
            member_instance,
            membership_credential: credential,
            resume_public_key: vec![3; 32],
            admission: AdmissionChangeFacts {
                member_instance,
                device_id,
                device_name: "joiner's laptop".into(),
                identity_fingerprint: joiner_fp(),
                transport_public_key: vec![5; 32],
                transport_address_blob: vec![],
                identity_signature: vec![6; 64],
            },
        }
    }

    /// 测试用 TTL：给大多数断言一个足够大的值，让 watchdog 在 test
    /// body 正常结束前绝不会 fire。需要验证 fire 的 test 自己用
    /// `tokio::time::pause` + `advance` 精确控制。
    const TEST_TTL: Duration = Duration::from_secs(3600);

    fn happy_coordinator(
        session_port: Arc<RecordingSessionPort>,
        space_access: Arc<MockSpaceAccess>,
        proof: Arc<ScriptedProof>,
        settings: Arc<StubSettings>,
    ) -> Arc<SponsorHandshakeCoordinator> {
        happy_coordinator_with_ttl(session_port, space_access, proof, settings, TEST_TTL)
    }

    fn happy_coordinator_with_ttl(
        session_port: Arc<RecordingSessionPort>,
        space_access: Arc<MockSpaceAccess>,
        proof: Arc<ScriptedProof>,
        _settings: Arc<StubSettings>,
        ttl: Duration,
    ) -> Arc<SponsorHandshakeCoordinator> {
        let delivery = noop_delivery();
        SponsorHandshakeCoordinator::new(
            session_port,
            space_access,
            delivery,
            proof,
            Arc::new(StubCurrentSpace),
            ttl,
        )
    }

    struct StubCurrentSpace;
    #[async_trait]
    impl CurrentSpaceIdentityPort for StubCurrentSpace {
        async fn current_space_id(
            &self,
        ) -> Result<Option<SpaceId>, crate::space::current_space::CurrentSpaceIdentityError>
        {
            Ok(Some(SpaceId::from("test-space")))
        }
    }

    fn happy_defaults() -> (
        Arc<RecordingSessionPort>,
        Arc<MockSpaceAccess>,
        Arc<ScriptedProof>,
        Arc<StubSettings>,
    ) {
        (
            Arc::new(RecordingSessionPort::default()),
            space_access(),
            Arc::new(ScriptedProof(StdMutex::new(vec![Ok(true)]))),
            Arc::new(StubSettings::named("sponsor-mac")),
        )
    }

    // ── begin ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn begin_sends_admission_offer_and_parks_ctx() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp.clone(), sa, pr, st);
        let session = PairingSessionId::new("s1");
        coord.begin(&session, joiner_request()).await.unwrap();

        let sent = sp.sent();
        assert_eq!(sent.len(), 1);
        match &sent[0].1 {
            PairingSessionMessage::AdmissionOffer(o) => {
                assert_eq!(o.space_id.inner(), "space-xyz");
                assert_eq!(o.kdf_parameters_blob, vec![0xAA; 32]);
                assert_eq!(o.challenge, vec![0x42; 32]);
                assert_eq!(o.pairing_session_id, session);
            }
            other => panic!("expected AdmissionOffer, got {other:?}"),
        }
        assert!(sp.closed().is_empty());
        assert_eq!(coord.parked_sessions().await, 1);
    }

    #[tokio::test]
    async fn begin_prepare_offer_failure_emits_internal_reject() {
        let (sp, _sa, pr, st) = happy_defaults();
        let sa = failing_space_access();
        let coord = happy_coordinator(sp.clone(), sa, pr, st);
        let session = PairingSessionId::new("s2");
        assert!(coord.begin(&session, joiner_request()).await.is_err());

        let sent = sp.sent();
        assert_eq!(sent.len(), 1);
        match &sent[0].1 {
            PairingSessionMessage::Reject(r) => match &r.reason {
                PairingRejectReason::Internal(m) => {
                    assert!(m.contains("prepare_admission_offer"), "msg = {m}")
                }
                o => panic!("expected Internal, got {o:?}"),
            },
            o => panic!("expected Reject, got {o:?}"),
        }
        assert_eq!(sp.closed().len(), 1);
        assert_eq!(coord.parked_sessions().await, 0);
    }

    #[tokio::test]
    async fn begin_keyslot_send_failure_closes_and_drops_ctx() {
        let (sp, sa, pr, st) = happy_defaults();
        *sp.fail_send.lock().unwrap() = true;
        let coord = happy_coordinator(sp.clone(), sa, pr, st);
        let session = PairingSessionId::new("s3");
        assert!(coord.begin(&session, joiner_request()).await.is_err());

        assert!(sp.sent().is_empty());
        assert_eq!(sp.closed().len(), 1);
        assert_eq!(coord.parked_sessions().await, 0);
    }

    // ── verify_challenge ────────────────────────────────────────────────

    #[tokio::test]
    async fn verify_returns_verified_with_joiner_facts() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp, sa, pr, st);
        let session = PairingSessionId::new("s4");
        coord.begin(&session, joiner_request()).await.unwrap();

        let v = coord
            .verify_challenge(
                &session,
                JoinerChallengeResponse {
                    encrypted_challenge: vec![0xFF],
                },
            )
            .await;
        match v {
            Some(Verdict::Verified(f)) => {
                assert_eq!(f.request.device_id.as_str(), "joiner-device");
                assert_eq!(f.request.device_name, "joiner's laptop");
                assert_eq!(f.request.identity_fingerprint, joiner_fp());
            }
            other => panic!("expected Verified, got {other:?}"),
        }
        assert_eq!(coord.parked_sessions().await, 1, "ctx kept for confirm");
    }

    #[tokio::test]
    async fn verify_returns_rejected_on_bad_proof() {
        let (sp, sa, _pr, st) = happy_defaults();
        let pr = Arc::new(ScriptedProof(StdMutex::new(vec![Ok(false)])));
        let coord = happy_coordinator(sp, sa, pr, st);
        let session = PairingSessionId::new("s5");
        coord.begin(&session, joiner_request()).await.unwrap();

        let v = coord
            .verify_challenge(
                &session,
                JoinerChallengeResponse {
                    encrypted_challenge: vec![],
                },
            )
            .await;
        assert!(matches!(v, Some(Verdict::Rejected)));
    }

    #[tokio::test]
    async fn verify_proof_port_error_is_treated_as_rejected() {
        let (sp, sa, _pr, st) = happy_defaults();
        let pr = Arc::new(ScriptedProof(StdMutex::new(vec![Err(anyhow::anyhow!(
            "x"
        ))])));
        let coord = happy_coordinator(sp, sa, pr, st);
        let session = PairingSessionId::new("s6");
        coord.begin(&session, joiner_request()).await.unwrap();
        let v = coord
            .verify_challenge(
                &session,
                JoinerChallengeResponse {
                    encrypted_challenge: vec![],
                },
            )
            .await;
        assert!(matches!(v, Some(Verdict::Rejected)));
    }

    #[tokio::test]
    async fn verify_without_parked_ctx_returns_none() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp, sa, pr, st);
        let v = coord
            .verify_challenge(
                &PairingSessionId::new("ghost"),
                JoinerChallengeResponse {
                    encrypted_challenge: vec![],
                },
            )
            .await;
        assert!(v.is_none());
    }

    // ── reject ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reject_sends_reject_wire_closes_drops_ctx() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp.clone(), sa, pr, st);
        let session = PairingSessionId::new("s9");
        coord.begin(&session, joiner_request()).await.unwrap();
        coord
            .reject(&session, PairingRejectReason::PassphraseMismatch)
            .await;

        let sent = sp.sent();
        assert_eq!(sent.len(), 2, "AdmissionOffer + Reject");
        match &sent[1].1 {
            PairingSessionMessage::Reject(r) => {
                assert_eq!(r.reason, PairingRejectReason::PassphraseMismatch)
            }
            other => panic!("expected Reject, got {other:?}"),
        }
        assert_eq!(sp.closed().len(), 1);
        assert_eq!(coord.parked_sessions().await, 0);
    }

    #[tokio::test]
    async fn reject_without_ctx_is_idempotent_and_still_closes() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp.clone(), sa, pr, st);
        coord
            .reject(
                &PairingSessionId::new("never-seen"),
                PairingRejectReason::InvitationMismatch,
            )
            .await;
        // Still emits Reject + close — the wire layer tolerates an id
        // with no active session.
        assert_eq!(sp.sent().len(), 1);
        assert_eq!(sp.closed().len(), 1);
    }

    // ── handle_session_closed ─────────────────────────────────────────

    #[tokio::test]
    async fn handle_session_closed_drops_parked_ctx() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp, sa, pr, st);
        let session = PairingSessionId::new("sc1");
        coord.begin(&session, joiner_request()).await.unwrap();
        assert_eq!(coord.parked_sessions().await, 1);
        coord
            .handle_session_closed(&session, Some("peer bailed"))
            .await;
        assert_eq!(coord.parked_sessions().await, 0);
    }

    #[tokio::test]
    async fn handle_session_closed_on_unknown_is_noop() {
        let (sp, sa, pr, st) = happy_defaults();
        let coord = happy_coordinator(sp, sa, pr, st);
        coord
            .handle_session_closed(&PairingSessionId::new("unknown"), None)
            .await;
        assert_eq!(coord.parked_sessions().await, 0);
    }

    // ── TTL watchdog (P7g) ─────────────────────────────────────────────
    //
    // 用 `start_paused = true` 让 tokio 时钟在 test 里只因
    // `tokio::time::advance` 才推进；这样 watchdog fire 的时机是确定的。
    // `sleep(ttl).await` + `advance(ttl)` 即能精确重放超时事件。

    #[tokio::test(start_paused = true)]
    async fn ttl_fires_reject_timeout_and_drops_ctx_when_no_response() {
        let (sp, sa, pr, st) = happy_defaults();
        let ttl = Duration::from_secs(30);
        let coord = happy_coordinator_with_ttl(sp.clone(), sa, pr, st, ttl);
        let session = PairingSessionId::new("timeout-1");
        coord.begin(&session, joiner_request()).await.unwrap();
        assert_eq!(coord.parked_sessions().await, 1);

        // `sleep` 在 paused clock 下让 runtime 自动推进到下个
        // 已登记 deadline（即 watchdog 的 sleep）。因此既推进时间又
        // 让 watchdog 的 `send_reject_and_close` 跑到头。
        tokio::time::sleep(ttl + Duration::from_secs(1)).await;

        let sent = sp.sent();
        assert_eq!(sent.len(), 2, "AdmissionOffer + Reject(Timeout)");
        match &sent[1].1 {
            PairingSessionMessage::Reject(r) => {
                assert_eq!(r.reason, PairingRejectReason::Timeout)
            }
            other => panic!("expected Reject, got {other:?}"),
        }
        assert_eq!(sp.closed().len(), 1);
        assert_eq!(
            coord.parked_sessions().await,
            0,
            "timeout fire must drop ctx"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn candidate_send_aborts_ttl_watchdog() {
        let (sp, sa, pr, st) = happy_defaults();
        let ttl = Duration::from_secs(30);
        let coord = happy_coordinator_with_ttl(sp.clone(), sa, pr, st, ttl);
        let session = PairingSessionId::new("candidate-abort");
        coord.begin(&session, joiner_request()).await.unwrap();
        let _ = coord
            .verify_challenge(
                &session,
                JoinerChallengeResponse {
                    encrypted_challenge: vec![],
                },
            )
            .await;
        coord
            .send_durable_candidate(
                &session,
                uc_core::pairing::DurableAdmissionFrame {
                    attempt_id: [1; 32],
                    kind: uc_core::pairing::DurableAdmissionMessageKind::Candidate,
                    message_id: [2; 32],
                    predecessor_message_id: Some([3; 32]),
                    payload: vec![4],
                },
            )
            .await
            .unwrap();
        coord.complete(&session).await;

        // 时间跨过 TTL，任何没被 abort 的 watchdog 都会在这一步 fire。
        tokio::time::sleep(ttl * 2).await;

        // 只应看到 AdmissionOffer + Candidate —— 绝不能多出 Reject。
        let sent = sp.sent();
        assert_eq!(sent.len(), 2, "AdmissionOffer + Candidate only");
        assert!(matches!(
            sent[0].1,
            PairingSessionMessage::AdmissionOffer(_)
        ));
        assert!(matches!(
            sent[1].1,
            PairingSessionMessage::DurableAdmission(ref frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::Candidate
        ));
        assert_eq!(sp.closed().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn reject_aborts_ttl_watchdog() {
        let (sp, sa, pr, st) = happy_defaults();
        let ttl = Duration::from_secs(30);
        let coord = happy_coordinator_with_ttl(sp.clone(), sa, pr, st, ttl);
        let session = PairingSessionId::new("reject-abort");
        coord.begin(&session, joiner_request()).await.unwrap();
        coord
            .reject(&session, PairingRejectReason::PassphraseMismatch)
            .await;

        tokio::time::sleep(ttl * 2).await;

        let sent = sp.sent();
        assert_eq!(sent.len(), 2, "AdmissionOffer + Reject(PassphraseMismatch)");
        match &sent[1].1 {
            PairingSessionMessage::Reject(r) => {
                assert_eq!(r.reason, PairingRejectReason::PassphraseMismatch)
            }
            other => panic!("expected Reject, got {other:?}"),
        }
        // 只该有一次 close —— watchdog 若真跑过会再发一次。
        assert_eq!(sp.closed().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn handle_session_closed_aborts_ttl_watchdog() {
        let (sp, sa, pr, st) = happy_defaults();
        let ttl = Duration::from_secs(30);
        let coord = happy_coordinator_with_ttl(sp.clone(), sa, pr, st, ttl);
        let session = PairingSessionId::new("closed-abort");
        coord.begin(&session, joiner_request()).await.unwrap();
        coord
            .handle_session_closed(&session, Some("peer disconnected"))
            .await;

        tokio::time::sleep(ttl * 2).await;

        // 仅 AdmissionOffer；handle_session_closed 本身不打 wire，
        // watchdog 若 leak 就会多出 Reject(Timeout)。
        let sent = sp.sent();
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0].1,
            PairingSessionMessage::AdmissionOffer(_)
        ));
        // handle_session_closed 不主动 close（transport 侧已 close），
        // 所以 closed 记录应为 0。
        assert!(sp.closed().is_empty());
    }
}
