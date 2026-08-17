//! Sponsor-side inbound pairing orchestrator.
//!
//! Internal communication implementation of workspace admission
//! (ADR-017). It chains three pieces into one sponsor-side pairing session:
//!
//! 1. **Pairing invitation** — `InMemoryPairingInvitationHolder::take_matching`
//!    + `PairingInvitationPort::consume_invitation` decide whether this
//!    inbound joiner is expected at all.
//! 2. **Handshake** — [`SponsorHandshakeCoordinator`] prepares the
//!    admission offer, parks per-session state, verifies the joiner's
//!    challenge response, and emits `Confirm` / `Reject` on the wire.
//! 3. **Workspace owner handover** — every decision (admit or reject) and
//!    every save boundary belongs to the workspace owner via
//!    [`super::super::adapter::WorkspaceAdmissionOwnerPort`]: the owner
//!    saves the in-flight admission record before the joiner's readiness,
//!    commits the admission change + pending handoff facts + confirmation
//!    material in one save commit when readiness arrives, and the channel
//!    only executes the returned decisions and sends the confirmation.
//!
//! Ordering matters: the workspace decision and the owner's saves run
//! **before** the wire `Confirm`, and the admission change commit runs
//! before the "admission change saved" reply, so the sponsor never tells
//! the joiner "you're in" after having failed to record it.
//!
//! Per `uc-application/AGENTS.md` §11.4 everything here is `pub(crate)`;
//! the facade constructs the orchestrator during `SpaceFacade::new`
//! and external callers reach pairing exclusively through that facade.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use chrono::{TimeZone, Utc};
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};
use uc_observability_contract::FlowId;

use uc_core::membership::MembershipAdmissionDecision;
use uc_core::pairing::invitation::InvitationCode;
use uc_core::pairing::session_message::{
    JoinerRequest, PairingRejectReason, PairingSessionMessage,
};
use uc_core::ports::pairing::{PairingEventPort, PairingSessionEvent, PairingSessionId};
use uc_core::ports::{ClockPort, ConsumeInvitationError, PairingInvitationPort};
use uc_observability_contract::analytics::{
    AnalyticsFacade, Event, PairingFailureReason, PairingMethod,
};

use super::sponsor_handshake::{JoinerFacts, SponsorHandshakeCoordinator, Verdict};
use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
use crate::space::admission::invitation::holder::{
    InMemoryPairingInvitationHolder, TakeMatchingError,
};
use crate::space::convergence::WorkspaceConvergenceError;

/// Drives sponsor-side inbound pairing events.
pub(crate) struct PairingInboundOrchestrator {
    pairing_events: Arc<dyn PairingEventPort>,
    pairing_invitation: Arc<dyn PairingInvitationPort>,
    holder: Arc<InMemoryPairingInvitationHolder>,
    clock: Arc<dyn ClockPort>,
    handshake: Arc<SponsorHandshakeCoordinator>,
    /// The workspace owner behind the admission seam. Never `None`: the
    /// assembly layer guarantees the owner always exists.
    workspace_convergence: Arc<dyn WorkspaceAdmissionOwnerPort>,
    /// Failure telemetry for `pairing_failed`. `pairing_started` is fired
    /// upstream by `IssuePairingInvitationUseCase`; the orchestrator no
    /// longer emits any pairing-success event.
    analytics: Arc<dyn AnalyticsFacade>,
    /// Per-session handshake start time, populated when the first valid
    /// `Request` arrives (`on_incoming` after invitation match). Failure
    /// paths drop their entry without consulting it. Bounded growth is
    /// guaranteed because every entry is removed at terminal (success or
    /// any post-match failure).
    handshake_started_at: Arc<StdMutex<HashMap<PairingSessionId, Instant>>>,
}

impl PairingInboundOrchestrator {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        pairing_events: Arc<dyn PairingEventPort>,
        pairing_invitation: Arc<dyn PairingInvitationPort>,
        holder: Arc<InMemoryPairingInvitationHolder>,
        clock: Arc<dyn ClockPort>,
        handshake: Arc<SponsorHandshakeCoordinator>,
        workspace_convergence: Arc<dyn WorkspaceAdmissionOwnerPort>,
        analytics: Arc<dyn AnalyticsFacade>,
    ) -> Self {
        Self {
            pairing_events,
            pairing_invitation,
            holder,
            clock,
            handshake,
            workspace_convergence,
            analytics,
            handshake_started_at: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Drop the per-session start time (success or failure terminal).
    fn take_started_at(&self, session: &PairingSessionId) -> Option<Instant> {
        self.handshake_started_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session)
    }

    /// Fire `pairing_failed` with structured reason. The session terminal
    /// state (reject, timeout, close) is an internal communication result
    /// and is not broadcast anywhere; the only outward expression of join
    /// results is the workspace state.
    fn emit_failure(&self, session: &PairingSessionId, reason: PairingFailureReason) {
        // Drop any started_at entry parked at on_incoming so the map stays
        // bounded even on the failure paths.
        let _ = self.take_started_at(session);
        self.analytics.capture(Event::PairingFailed {
            method: PairingMethod::Code,
            failure_reason: reason,
        });
    }

    /// Subscribe to the event port and spawn the drain loop. Returned
    /// `JoinHandle` is owned by the facade so shutdown can `abort()`.
    pub(crate) fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let rx = match self.pairing_events.subscribe().await {
                Ok(rx) => rx,
                Err(err) => {
                    warn!(
                        error = %err,
                        "pairing inbound orchestrator failed to subscribe; task exiting"
                    );
                    return;
                }
            };
            self.run_loop(rx).await;
        })
    }

    async fn run_loop(self: Arc<Self>, mut rx: Receiver<PairingSessionEvent>) {
        info!("pairing inbound orchestrator started");
        while let Some(event) = rx.recv().await {
            self.handle_event(event).await;
        }
        info!("pairing inbound orchestrator stopped (event channel closed)");
    }

    // 跨设备可观测性(PR2):
    //   - root span 一开 session 就能拿到 `session.id`,直接做静态字段;
    //   - `flow.id` / `peer.device_id` 在配对入口阶段还不知道(joiner 提交
    //     Request 后才能确定),声明为 `tracing::field::Empty` 占位,在
    //     `match_invitation` / `finalise_verified` 等下游方法里用
    //     `Span::current().record(...)` 回填 —— 因为这些方法都在
    //     `handle_event` 的 instrument 范围内,Span::current() 等价于本 root。
    //   - `flow.kind = "pairing"` 静态枚举值。
    #[instrument(
        skip_all,
        fields(
            event = event_kind(&event),
            session.id = %event_session_id(&event),
            flow.id = tracing::field::Empty,
            flow.kind = "pairing",
            peer.device_id = tracing::field::Empty,
        ),
    )]
    pub(crate) async fn handle_event(&self, event: PairingSessionEvent) {
        let flow_id = FlowId::generate();
        tracing::Span::current().record("flow.id", tracing::field::display(&flow_id));
        match event {
            PairingSessionEvent::Incoming { session, message } => {
                self.on_incoming(session, message).await
            }
            PairingSessionEvent::MessageReceived { session, message } => {
                self.on_message_received(session, message).await
            }
            PairingSessionEvent::Closed { session, reason } => {
                self.handshake
                    .handle_session_closed(&session, reason.as_deref())
                    .await;
            }
        }
    }

    async fn on_incoming(&self, session: PairingSessionId, message: PairingSessionMessage) {
        let incoming_variant = variant_name(&message);
        info!(
            session = %session,
            message_kind = incoming_variant,
            "inbound pairing event received"
        );
        if let PairingSessionMessage::DurableAdmission(frame) = message.clone() {
            if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::CompleteAck {
                self.handle_durable_admission(&session, frame).await;
                return;
            }
        }
        let request = match message {
            PairingSessionMessage::Request(req) => req,
            other => {
                warn!(
                    session = %session,
                    variant = variant_name(&other),
                    "first pairing message was not Request; rejecting session"
                );
                self.handshake
                    .reject(
                        &session,
                        PairingRejectReason::Internal(
                            "expected Request as first pairing message".into(),
                        ),
                    )
                    .await;
                return;
            }
        };
        info!(
            session = %session,
            code = %request.invitation_code.as_str(),
            joiner_device_id = %request.device_id.as_str(),
            transport_address_blob_len = request.transport_address_blob.len(),
            "inbound pairing Request received; matching invitation"
        );

        if let Err(error) = self
            .workspace_convergence
            .validate_join_request(&request)
            .await
        {
            warn!(session = %session, error = %error, "invalid durable join request");
            self.handshake
                .reject(
                    &session,
                    PairingRejectReason::Internal("invalid durable join request".into()),
                )
                .await;
            self.emit_failure(&session, PairingFailureReason::Internal);
            return;
        }

        let Some((_invitation_code, _generation)) = self.match_invitation(&session, &request).await
        else {
            return;
        };

        // Slice 8b' · stamp the per-session start time so the verified
        // path can compute handshake duration. Idempotent on re-entry:
        // the second insert silently overwrites — this only happens if
        // `Incoming` is replayed for the same session, which would
        // already be a protocol violation upstream.
        self.handshake_started_at
            .lock()
            .unwrap()
            .insert(session.clone(), Instant::now());

        // `begin` sends the AdmissionOffer + parks per-session state; on
        // failure it has already emitted Reject + close internally.
        match self.handshake.begin(&session, request).await {
            Ok(()) => info!(
                session = %session,
                "inbound pairing AdmissionOffer sent; waiting for ChallengeResponse"
            ),
            Err(()) => warn!(
                session = %session,
                "inbound pairing failed while sending AdmissionOffer"
            ),
        }
    }

    /// Returns the matched invitation code and its admission generation on
    /// success. On miss / expiry / holder invariant violation emits
    /// `Reject` via the handshake coordinator and returns `None`.
    async fn match_invitation(
        &self,
        session: &PairingSessionId,
        request: &JoinerRequest,
    ) -> Option<(InvitationCode, u64)> {
        let now_ms = self.clock.now_ms();
        let now = match Utc.timestamp_millis_opt(now_ms).single() {
            Some(ts) => ts,
            None => {
                warn!(
                    session = %session,
                    now_ms,
                    "ClockPort returned out-of-range timestamp; treating inbound as internal"
                );
                self.handshake
                    .reject(
                        session,
                        PairingRejectReason::Internal("sponsor clock out of range".into()),
                    )
                    .await;
                return None;
            }
        };

        match self
            .holder
            .take_matching(&request.invitation_code, now)
            .await
        {
            Ok(invitation) => {
                let generation = invitation.admission_generation();
                if self
                    .workspace_convergence
                    .admission_decision_for_joiner(generation, &request.device_id)
                    .await
                    != MembershipAdmissionDecision::Allowed
                {
                    // An old or currently blocked invitation must not disclose the
                    // space's current removal state before constructing an admission offer.
                    self.handshake
                        .reject(session, PairingRejectReason::AdmissionUnavailable)
                        .await;
                    self.emit_failure(session, PairingFailureReason::Internal);
                    return None;
                }
                // 把 joiner_device_id 提到 root span 的 `peer.device_id`,
                // 后续所有 child span / event 都自动继承,Sentry 上同一
                // pairing flow 的事件可以一键 filter 出来。
                tracing::Span::current().record(
                    "peer.device_id",
                    tracing::field::display(&request.device_id.as_str()),
                );
                info!(
                    session = %session,
                    code = %invitation.code().as_str(),
                    joiner_device_id = %request.device_id.as_str(),
                    "accepted joiner request for pending invitation"
                );
                Some((invitation.code().clone(), generation))
            }
            Err(TakeMatchingError::NotFound) => {
                info!(
                    session = %session,
                    code = %request.invitation_code.as_str(),
                    "inbound pairing request for unknown code; rejecting"
                );
                self.handshake
                    .reject(session, PairingRejectReason::InvitationMismatch)
                    .await;
                None
            }
            Err(TakeMatchingError::Expired) => {
                info!(
                    session = %session,
                    code = %request.invitation_code.as_str(),
                    "inbound pairing request after invitation expired; rejecting"
                );
                self.handshake
                    .reject(session, PairingRejectReason::InvitationMismatch)
                    .await;
                // Expired = our invitation; outer caller is done.
                self.emit_failure(session, PairingFailureReason::InvitationExpired);
                None
            }
            Err(TakeMatchingError::Internal(msg)) => {
                warn!(
                    session = %session,
                    code = %request.invitation_code.as_str(),
                    error = %msg,
                    "holder invariant broken on inbound pairing request; rejecting"
                );
                self.handshake
                    .reject(session, PairingRejectReason::Internal(msg))
                    .await;
                self.emit_failure(session, PairingFailureReason::Internal);
                None
            }
        }
    }

    async fn on_message_received(&self, session: PairingSessionId, message: PairingSessionMessage) {
        let message_variant = variant_name(&message);
        info!(
            session = %session,
            message_kind = message_variant,
            "inbound pairing follow-up message received"
        );
        if let PairingSessionMessage::DurableAdmission(frame) = message {
            self.handle_durable_admission(&session, frame).await;
            return;
        }
        let PairingSessionMessage::ChallengeResponse(response) = message else {
            // Anything else on a mid-handshake session is a joiner-side
            // protocol violation. Log without closing — the session
            // naturally resolves via a later Close or the joiner's own
            // Reject.
            info!(
                session = %session,
                variant = variant_name(&message),
                "unexpected mid-handshake message from joiner"
            );
            return;
        };

        let Some(verdict) = self.handshake.verify_challenge(&session, response).await else {
            debug!(
                session = %session,
                "ChallengeResponse arrived with no parked handshake ctx; ignoring"
            );
            return;
        };

        match verdict {
            Verdict::Verified(facts) => self.finalise_verified(&session, facts).await,
            Verdict::Rejected => {
                info!(session = %session, "joiner proof rejected; sending PassphraseMismatch");
                self.handshake
                    .reject(&session, PairingRejectReason::PassphraseMismatch)
                    .await;
                self.emit_failure(&session, PairingFailureReason::PassphraseMismatch);
            }
        }
    }

    async fn handle_durable_admission(
        &self,
        session: &PairingSessionId,
        frame: uc_core::pairing::DurableAdmissionFrame,
    ) {
        match frame.kind {
            uc_core::pairing::DurableAdmissionMessageKind::Prepared => {
                match self
                    .workspace_convergence
                    .commit_sponsor_prepared(&frame)
                    .await
                {
                    Ok(commit) => {
                        if let Err(error) = self.handshake.send_durable_frame(session, commit).await
                        {
                            warn!(session = %session, error = %error, "Commit send failed after durable save");
                        }
                    }
                    Err(error) => {
                        warn!(session = %session, error = %error, "Prepared verification failed");
                        self.handshake
                            .reject(
                                session,
                                PairingRejectReason::Internal(format!(
                                    "commit_sponsor_prepared: {error}"
                                )),
                            )
                            .await;
                    }
                }
            }
            uc_core::pairing::DurableAdmissionMessageKind::Applied => {
                match self
                    .workspace_convergence
                    .complete_sponsor_applied(&frame)
                    .await
                {
                    Ok(complete) => {
                        if let Err(error) =
                            self.handshake.send_durable_frame(session, complete).await
                        {
                            warn!(session = %session, error = %error, "Complete send failed after durable save");
                        }
                    }
                    Err(error) => {
                        warn!(session = %session, error = %error, "Applied verification failed");
                        self.handshake
                            .reject(
                                session,
                                PairingRejectReason::Internal(format!(
                                    "complete_sponsor_applied: {error}"
                                )),
                            )
                            .await;
                    }
                }
            }
            uc_core::pairing::DurableAdmissionMessageKind::CompleteAck => {
                match self
                    .workspace_convergence
                    .confirm_sponsor_complete_ack(&frame)
                    .await
                {
                    Ok(()) => {
                        self.handshake.complete(session).await;
                        info!(session = %session, "durable admission completed on both devices");
                    }
                    Err(error) => {
                        warn!(session = %session, error = %error, "CompleteAck verification failed");
                    }
                }
            }
            other => {
                warn!(session = %session, ?other, "unexpected durable admission frame at sponsor");
            }
        }
    }

    /// Verified branch: the owner saves the complete Candidate before the
    /// channel sends it. A retry therefore replays the same durable message.
    async fn finalise_verified(&self, session: &PairingSessionId, facts: JoinerFacts) {
        // Pre-admission chain synchronization: pull the local chain head up
        // to the newest known member so the admission change is appended to
        // a current head instead of forking the chain on a stale one.
        // Best effort: a failed or timed-out sync does not block the join
        // (receivers still reject forked changes on digest mismatch).
        if let Err(error) = self.workspace_convergence.synchronize_chain().await {
            warn!(
                session = %session,
                error = %error,
                "pre-admission chain synchronization incomplete; proceeding best-effort"
            );
        }
        let candidate = match self
            .workspace_convergence
            .prepare_sponsor_candidate(&facts.request)
            .await
        {
            Ok(candidate) => candidate,
            Err(error) => {
                warn!(
                    session = %session,
                    error = %error,
                    "durable Candidate preparation failed; rejecting"
                );
                let (reason, failure) =
                    if matches!(error, WorkspaceConvergenceError::AdmissionConflict) {
                        (
                            PairingRejectReason::AdmissionConflict,
                            PairingFailureReason::SponsorAdmissionConflict,
                        )
                    } else {
                        (
                            PairingRejectReason::Internal(format!(
                                "prepare_sponsor_candidate: {error}"
                            )),
                            PairingFailureReason::Internal,
                        )
                    };
                self.handshake.reject(session, reason).await;
                self.emit_failure(session, failure);
                return;
            }
        };
        if let Err(error) = self
            .handshake
            .send_durable_candidate(session, candidate)
            .await
        {
            warn!(session = %session, error = %error, "Candidate send failed after durable save");
            self.emit_failure(session, PairingFailureReason::ConnectionLost);
            return;
        }
        self.notify_consume(&facts.request.invitation_code).await;
    }

    async fn notify_consume(&self, code: &InvitationCode) {
        match self.pairing_invitation.consume_invitation(code).await {
            Ok(()) => debug!(code = %code.as_str(), "rendezvous consume acknowledged"),
            Err(ConsumeInvitationError::NotFound | ConsumeInvitationError::Expired) => debug!(
                code = %code.as_str(),
                "rendezvous entry already terminal on consume (benign)"
            ),
            Err(err) => warn!(
                code = %code.as_str(),
                error = %err,
                "rendezvous consume failed; local handshake proceeds regardless"
            ),
        }
    }
}

fn event_kind(event: &PairingSessionEvent) -> &'static str {
    match event {
        PairingSessionEvent::Incoming { .. } => "Incoming",
        PairingSessionEvent::MessageReceived { .. } => "MessageReceived",
        PairingSessionEvent::Closed { .. } => "Closed",
    }
}

/// 抽出当前 pairing 事件所属的 `session_id`。
///
/// 所有变体都自带 session,所以可以无条件返回 `&PairingSessionId`,
/// 让 `handle_event` 的 root span 把 `session.id` 直接做静态字段而不必
/// 用 `Empty` 占位再回填。
fn event_session_id(event: &PairingSessionEvent) -> &PairingSessionId {
    match event {
        PairingSessionEvent::Incoming { session, .. } => session,
        PairingSessionEvent::MessageReceived { session, .. } => session,
        PairingSessionEvent::Closed { session, .. } => session,
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
    //! The channel side of the admission seam (ADR-017): the orchestrator
    //! is verified against a workspace-owner double, so no real owner or
    //! real network is involved. The ordering contract under test is:
    //!
    //! match → consume → handshake.begin → verify → owner.begin_admission
    //! → confirm → Ready → owner.commit_joiner_admission →
    //! AdmissionSaved → close.
    //!
    //! The handshake wire adapter is covered in `sponsor_handshake::tests`;
    //! the owner's own save boundaries in
    //! `crate::space::convergence::tests`. Here we scope to the
    //! composition glue: which branches call the owner in which order,
    //! and that no member state is saved by the channel itself.
    use super::*;

    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};

    use uc_core::ids::{DeviceId, SessionId, SpaceId};
    use uc_core::membership::{AdmissionChangeFacts, MembershipAdmissionDecision};
    use uc_core::pairing::invitation::{InvitationCode, PairingInvitation};
    use uc_core::pairing::session_message::{
        JoinerChallengeResponse, PairingReject, PairingSecurityCapability,
    };
    use uc_core::ports::pairing::{DialError, DialOutcome, PairingSessionPort, SessionError};
    use uc_core::ports::pairing_invitation::{InvitationError, IssuedInvitation};
    use uc_core::ports::space::{PrepareAdmissionOfferPort, ProofPort, SpaceAccessError};
    use uc_core::ports::{
        ClockPort, ConsumeInvitationError, PairingInvitationPort, SetupStatusPort,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::space_access::domain::{
        PreparedAdmissionOffer, ProofDerivedKey, SpaceAccessProofArtifact,
    };
    use uc_observability_contract::analytics::{
        AnalyticsFacade, AnalyticsPort, DefaultAnalyticsFacade, NoopAnalyticsIdentity,
    };

    use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
    use crate::space::admission::invitation::holder::InMemoryPairingInvitationHolder;
    use crate::space::convergence::WorkspaceConvergenceError;

    use crate::space::convergence::membership::group_update_delivery::GroupUpdateDeliveryPort;

    // ── fakes ────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct CapturingAnalyticsSink {
        captured: StdMutex<Vec<Event>>,
    }
    impl CapturingAnalyticsSink {
        fn events(&self) -> Vec<Event> {
            self.captured.lock().unwrap().clone()
        }
    }
    impl AnalyticsPort for CapturingAnalyticsSink {
        fn capture(&self, event: Event) {
            self.captured.lock().unwrap().push(event);
        }
    }

    struct FakeClock(i64);
    impl ClockPort for FakeClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    /// Workspace-owner double behind the admission seam: records every call
    /// in order and lets tests script the admission decision and failures.
    struct RecordingOwner {
        calls: StdMutex<Vec<&'static str>>,
        decision: MembershipAdmissionDecision,
        fail_validate: bool,
    }
    impl RecordingOwner {
        fn allowed() -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                decision: MembershipAdmissionDecision::Allowed,
                fail_validate: false,
            }
        }
        fn with_decision(decision: MembershipAdmissionDecision) -> Self {
            Self {
                decision,
                ..Self::allowed()
            }
        }
        fn with_fail_validate() -> Self {
            Self {
                fail_validate: true,
                ..Self::allowed()
            }
        }
        fn call_log(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for RecordingOwner {
        async fn validate_join_request(
            &self,
            request: &JoinerRequest,
        ) -> Result<(), WorkspaceConvergenceError> {
            request
                .validate_durable_identity()
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_owned()))?;
            if self.fail_validate {
                self.calls.lock().unwrap().push("validate_join_request");
                return Err(WorkspaceConvergenceError::InvalidConfirmation);
            }
            Ok(())
        }

        async fn admission_decision_for_joiner(
            &self,
            _: u64,
            _: &DeviceId,
        ) -> MembershipAdmissionDecision {
            self.calls.lock().unwrap().push("admission_decision");
            self.decision
        }
        async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("synchronize_chain");
            Ok(())
        }

        async fn prepare_sponsor_candidate(
            &self,
            _request: &JoinerRequest,
        ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("prepare_sponsor_candidate");
            Ok(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Candidate,
            ))
        }

        async fn commit_sponsor_prepared(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
        ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("commit_sponsor_prepared");
            Ok(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Commit,
            ))
        }

        async fn complete_sponsor_applied(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
        ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("complete_sponsor_applied");
            Ok(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Complete,
            ))
        }

        async fn confirm_sponsor_complete_ack(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
        ) -> Result<(), WorkspaceConvergenceError> {
            self.calls
                .lock()
                .unwrap()
                .push("confirm_sponsor_complete_ack");
            Ok(())
        }
    }

    fn durable_frame(
        kind: uc_core::pairing::DurableAdmissionMessageKind,
    ) -> uc_core::pairing::DurableAdmissionFrame {
        uc_core::pairing::DurableAdmissionFrame {
            attempt_id: [0x31; 32],
            kind,
            message_id: [kind as u8; 32],
            predecessor_message_id: Some([0x32; 32]),
            payload: vec![kind as u8],
        }
    }

    #[derive(Default)]
    struct RecordingSessionPort {
        sent: StdMutex<Vec<(PairingSessionId, PairingSessionMessage)>>,
        closed: StdMutex<Vec<(PairingSessionId, Option<String>)>>,
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
        async fn dial_by_invitation(&self, _: &InvitationCode) -> Result<DialOutcome, DialError> {
            unimplemented!()
        }
        async fn send(
            &self,
            session: &PairingSessionId,
            message: PairingSessionMessage,
        ) -> Result<(), SessionError> {
            self.sent.lock().unwrap().push((session.clone(), message));
            Ok(())
        }
        async fn recv_next(
            &self,
            _: &PairingSessionId,
        ) -> Result<Option<PairingSessionMessage>, SessionError> {
            unimplemented!()
        }
        async fn close(&self, session: &PairingSessionId, reason: Option<String>) {
            self.closed.lock().unwrap().push((session.clone(), reason));
        }
    }

    struct ScriptedEventPort(StdMutex<Option<Receiver<PairingSessionEvent>>>);
    #[async_trait]
    impl PairingEventPort for ScriptedEventPort {
        async fn subscribe(&self) -> anyhow::Result<Receiver<PairingSessionEvent>> {
            self.0
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| anyhow::anyhow!("already subscribed"))
        }
    }

    #[derive(Default)]
    struct RecordingInvitationPort {
        consumed: StdMutex<Vec<InvitationCode>>,
    }
    #[async_trait]
    impl PairingInvitationPort for RecordingInvitationPort {
        async fn issue_invitation(&self) -> Result<IssuedInvitation, InvitationError> {
            unimplemented!()
        }
        async fn consume_invitation(
            &self,
            code: &InvitationCode,
        ) -> Result<(), ConsumeInvitationError> {
            self.consumed.lock().unwrap().push(code.clone());
            Ok(())
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

    mockall::mock! {
        SpaceAccess {}

        #[async_trait]
        impl PrepareAdmissionOfferPort for SpaceAccess {
            async fn prepare_admission_offer(
                &self,
                space_id: &SpaceId,
                invitation: &InvitationCode,
                pairing_session_id: &SessionId,
            ) -> Result<PreparedAdmissionOffer, SpaceAccessError>;
        }

    }

    fn noop_delivery() -> Arc<MockGroupUpdateDelivery> {
        let mut delivery = MockGroupUpdateDelivery::new();
        delivery.expect_deliver_pending().returning(|_| Ok(0));
        Arc::new(delivery)
    }

    fn sponsor_space_access() -> Arc<MockSpaceAccess> {
        let mut mock = MockSpaceAccess::new();
        mock.expect_prepare_admission_offer().returning(|_, _, _| {
            Ok(PreparedAdmissionOffer {
                offer: uc_core::space_access::AdmissionOffer {
                    space_id: SpaceId::from_str("space-xyz"),
                    kdf_parameters_blob: vec![0xAA; 32],
                    challenge_nonce: [0x42; 32],
                },
                verification_key: ProofDerivedKey::from_bytes([0x55; 32]),
            })
        });
        Arc::new(mock)
    }

    struct ScriptedProof(StdMutex<Vec<bool>>);
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
            Ok(if q.is_empty() { false } else { q.remove(0) })
        }
    }

    struct OrchestratorStubSetupStatus;
    #[async_trait]
    impl SetupStatusPort for OrchestratorStubSetupStatus {
        async fn get_status(&self) -> anyhow::Result<uc_core::setup::SetupStatus> {
            Ok(uc_core::setup::SetupStatus {
                has_completed: true,
                space_id: None,
            })
        }
        async fn set_status(&self, _s: &uc_core::setup::SetupStatus) -> anyhow::Result<()> {
            Ok(())
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-04-20T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
    fn fixed_now_ms() -> i64 {
        fixed_now().timestamp_millis()
    }
    fn joiner_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("AAAAAAAAAAAAAAAA").unwrap()
    }
    fn pending(code: &str) -> PairingInvitation {
        let issued = fixed_now();
        let expires = issued + Duration::minutes(5);
        let (inv, _) = PairingInvitation::issue(
            InvitationCode::new(code),
            issued,
            expires,
            DeviceId::new("sponsor-1"),
            0,
        );
        inv
    }
    fn joiner_request(code: &str) -> JoinerRequest {
        let admission = joiner_facts();
        JoinerRequest {
            attempt_id: [1; 32],
            join_id: [2; 16],
            request_message_id: [3; 32],
            invitation_code: InvitationCode::new(code),
            device_id: DeviceId::new("joiner-device"),
            device_name: "joiner's laptop".into(),
            identity_fingerprint: joiner_fp(),
            nonce: vec![1, 2, 3, 4],
            transport_address_blob: vec![],
            security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
            key_package: vec![1, 2, 3],
            member_instance: admission.member_instance,
            membership_credential: joiner_credential(),
            resume_public_key: vec![3; 32],
            admission,
        }
    }

    fn joiner_credential() -> uc_core::membership::MembershipCredential {
        uc_core::membership::MembershipCredential::new(1, vec![4; 32])
    }

    fn joiner_facts() -> AdmissionChangeFacts {
        let device_id = DeviceId::new("joiner-device");
        AdmissionChangeFacts {
            member_instance: joiner_credential().member_instance_id(&device_id),
            device_id,
            device_name: "joiner's laptop".into(),
            identity_fingerprint: joiner_fp(),
            transport_public_key: vec![1; 32],
            transport_address_blob: vec![],
            identity_signature: vec![2; 64],
        }
    }

    struct Bundle {
        session_port: Arc<RecordingSessionPort>,
        invitation_port: Arc<RecordingInvitationPort>,
        holder: Arc<InMemoryPairingInvitationHolder>,
        proof_verdicts: Vec<bool>,
        clock_ms: i64,
        owner: Arc<RecordingOwner>,
        analytics: Arc<dyn AnalyticsPort>,
    }

    impl Bundle {
        fn happy() -> Self {
            Self {
                session_port: Arc::new(RecordingSessionPort::default()),
                invitation_port: Arc::new(RecordingInvitationPort::default()),
                holder: Arc::new(InMemoryPairingInvitationHolder::new()),
                proof_verdicts: vec![true],
                clock_ms: fixed_now_ms(),
                owner: Arc::new(RecordingOwner::allowed()),
                analytics: Arc::new(uc_observability_contract::analytics::NoopAnalyticsSink),
            }
        }

        fn build(
            self,
        ) -> (
            Arc<PairingInboundOrchestrator>,
            Arc<RecordingSessionPort>,
            Arc<RecordingOwner>,
        ) {
            let space_access = sponsor_space_access();
            let handshake = SponsorHandshakeCoordinator::new(
                self.session_port.clone() as Arc<dyn PairingSessionPort>,
                space_access,
                noop_delivery(),
                Arc::new(ScriptedProof(StdMutex::new(self.proof_verdicts))),
                Arc::new(OrchestratorStubSetupStatus),
                std::time::Duration::from_secs(3600),
            );
            let orch = Arc::new(PairingInboundOrchestrator::new(
                Arc::new(ScriptedEventPort(StdMutex::new(None))),
                self.invitation_port.clone(),
                self.holder.clone(),
                Arc::new(FakeClock(self.clock_ms)) as Arc<dyn ClockPort>,
                handshake,
                Arc::clone(&self.owner) as Arc<dyn WorkspaceAdmissionOwnerPort>,
                Arc::new(DefaultAnalyticsFacade::new(
                    self.analytics.clone(),
                    Arc::new(NoopAnalyticsIdentity),
                )) as Arc<dyn AnalyticsFacade>,
            ));
            (orch, self.session_port, Arc::clone(&self.owner))
        }
    }

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn invalid_durable_request_does_not_consume_the_invitation() {
        let mut bundle = Bundle::happy();
        bundle.owner = Arc::new(RecordingOwner::with_fail_validate());
        bundle.holder.insert(pending("CODE-1")).await;
        let holder = Arc::clone(&bundle.holder);
        let invitation_port = Arc::clone(&bundle.invitation_port);
        let (orch, session_port, owner) = bundle.build();
        let session = PairingSessionId::new("session-1");

        orch.handle_event(PairingSessionEvent::Incoming {
            session,
            message: PairingSessionMessage::Request(joiner_request("CODE-1")),
        })
        .await;

        assert_eq!(owner.call_log(), vec!["validate_join_request"]);
        assert!(invitation_port.consumed.lock().unwrap().is_empty());
        assert!(matches!(
            session_port.sent().last().map(|(_, message)| message),
            Some(PairingSessionMessage::Reject(PairingReject {
                reason: PairingRejectReason::Internal(_),
            }))
        ));
        assert!(holder
            .take_matching(
                &InvitationCode::new("CODE-1"),
                Utc.timestamp_millis_opt(fixed_now_ms()).single().unwrap(),
            )
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn happy_path_saves_each_durable_stage_before_sending_the_next() {
        let bundle = Bundle::happy();
        bundle.holder.insert(pending("CODE-1")).await;
        let (orch, session_port, owner) = bundle.build();
        let session = PairingSessionId::new("session-1");
        orch.handle_event(PairingSessionEvent::Incoming {
            session: session.clone(),
            message: PairingSessionMessage::Request(joiner_request("CODE-1")),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::ChallengeResponse(JoinerChallengeResponse {
                encrypted_challenge: vec![0xAB],
            }),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::DurableAdmission(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Prepared,
            )),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::DurableAdmission(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Applied,
            )),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::DurableAdmission(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::CompleteAck,
            )),
        })
        .await;

        let sent = session_port.sent();
        assert!(matches!(
            sent[0].1,
            PairingSessionMessage::AdmissionOffer(_)
        ));
        let durable_kinds = sent
            .iter()
            .filter_map(|(_, message)| match message {
                PairingSessionMessage::DurableAdmission(frame) => Some(frame.kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            durable_kinds,
            vec![
                uc_core::pairing::DurableAdmissionMessageKind::Candidate,
                uc_core::pairing::DurableAdmissionMessageKind::Commit,
                uc_core::pairing::DurableAdmissionMessageKind::Complete,
            ]
        );
        assert_eq!(session_port.closed().len(), 1);
        let calls = owner.call_log();
        assert_eq!(
            calls,
            vec![
                "admission_decision",
                "synchronize_chain",
                "prepare_sponsor_candidate",
                "commit_sponsor_prepared",
                "complete_sponsor_applied",
                "confirm_sponsor_complete_ack",
            ]
        );
    }

    #[tokio::test]
    async fn reopened_admission_channel_accepts_complete_ack_as_its_first_message() {
        let bundle = Bundle::happy();
        let (orchestrator, session_port, owner) = bundle.build();
        let session = PairingSessionId::new("continued-session");

        orchestrator
            .handle_event(PairingSessionEvent::Incoming {
                session: session.clone(),
                message: PairingSessionMessage::DurableAdmission(durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::CompleteAck,
                )),
            })
            .await;

        assert_eq!(owner.call_log(), vec!["confirm_sponsor_complete_ack"]);
        assert!(session_port.sent().is_empty());
        assert_eq!(session_port.closed().len(), 1);
    }

    #[tokio::test]
    async fn owner_rejects_admission_with_reject_and_no_save() {
        let mut bundle = Bundle::happy();
        bundle.owner = Arc::new(RecordingOwner::with_decision(
            MembershipAdmissionDecision::SupersededInvitation,
        ));
        bundle.holder.insert(pending("CODE-1")).await;
        let (orch, session_port, owner) = bundle.build();
        orch.handle_event(PairingSessionEvent::Incoming {
            session: PairingSessionId::new("session-1"),
            message: PairingSessionMessage::Request(joiner_request("CODE-1")),
        })
        .await;
        let sent = session_port.sent();
        assert!(
            matches!(
                sent.last().map(|(_, m)| m),
                Some(PairingSessionMessage::Reject(PairingReject {
                    reason: PairingRejectReason::AdmissionUnavailable,
                }))
            ),
            "expected AdmissionUnavailable reject, got {sent:?}"
        );
        assert_eq!(
            owner.call_log(),
            vec!["admission_decision"],
            "no save boundary crossed on a rejected admission"
        );
    }

    #[tokio::test]
    async fn unmatched_invitation_rejects_without_owner_calls() {
        let bundle = Bundle::happy();
        let (orch, session_port, owner) = bundle.build();
        orch.handle_event(PairingSessionEvent::Incoming {
            session: PairingSessionId::new("session-1"),
            message: PairingSessionMessage::Request(joiner_request("UNKNOWN-CODE")),
        })
        .await;
        assert!(
            matches!(
                session_port.sent().last().map(|(_, m)| m),
                Some(PairingSessionMessage::Reject(PairingReject {
                    reason: PairingRejectReason::InvitationMismatch,
                }))
            ),
            "expected InvitationMismatch reject"
        );
        assert!(owner.call_log().is_empty());
    }

    #[tokio::test]
    async fn success_path_emits_no_pairing_success_analytics() {
        let mut bundle = Bundle::happy();
        let sink = Arc::new(CapturingAnalyticsSink::default());
        bundle.analytics = sink.clone();
        bundle.holder.insert(pending("CODE-1")).await;
        let (orch, _session_port, _owner) = bundle.build();
        let session = PairingSessionId::new("session-1");
        orch.handle_event(PairingSessionEvent::Incoming {
            session: session.clone(),
            message: PairingSessionMessage::Request(joiner_request("CODE-1")),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::ChallengeResponse(JoinerChallengeResponse {
                encrypted_challenge: vec![0xAB],
            }),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::DurableAdmission(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Prepared,
            )),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::DurableAdmission(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Applied,
            )),
        })
        .await;
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session,
            message: PairingSessionMessage::DurableAdmission(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::CompleteAck,
            )),
        })
        .await;
        assert!(
            sink.events().is_empty(),
            "no analytics events after a committed admission (success is expressed by the workspace state)"
        );
    }
}
