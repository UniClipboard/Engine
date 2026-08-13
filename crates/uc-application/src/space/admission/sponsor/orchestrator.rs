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
    JoinerReady, JoinerRequest, PairingRejectReason, PairingSessionMessage,
};
use uc_core::ports::pairing::{PairingEventPort, PairingSessionEvent, PairingSessionId};
use uc_core::ports::{ClockPort, ConsumeInvitationError, PairingInvitationPort};
use uc_observability_contract::analytics::{
    AnalyticsFacade, Event, PairingFailureReason, PairingMethod,
};

use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
use crate::space::admission::invitation::holder::{
    InMemoryPairingInvitationHolder, TakeMatchingError,
};
use crate::space::convergence::WorkspaceConvergenceError;

use super::sponsor_handshake::{JoinerFacts, SponsorHandshakeCoordinator, Verdict};

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
    pending_joiner_ready: Arc<StdMutex<HashMap<PairingSessionId, JoinerFacts>>>,
    /// Admission generation the matched invitation was bound to, parked per
    /// session so the owner can save the in-flight admission record with the
    /// correct generation once the challenge passes.
    pending_generation: Arc<StdMutex<HashMap<PairingSessionId, u64>>>,
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
            pending_joiner_ready: Arc::new(StdMutex::new(HashMap::new())),
            pending_generation: Arc::new(StdMutex::new(HashMap::new())),
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
        self.pending_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session);
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

        let Some((invitation_code, generation)) = self.match_invitation(&session, &request).await
        else {
            return;
        };
        self.notify_consume(&invitation_code).await;

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
        // The verified joiner's admission record is saved by the owner
        // once the challenge passes; the generation is stashed per session
        // for `finalise_verified`.
        self.pending_generation
            .lock()
            .unwrap()
            .insert(session.clone(), generation);
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
        if let PairingSessionMessage::Ready(ready) = message {
            self.complete_after_joiner_ready(&session, ready).await;
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

    /// Verified branch: the owner saves the in-flight admission record,
    /// then the channel sends `Confirm`. Any owner error short-circuits to
    /// `Reject(Internal)` so the joiner never sees a false Confirm.
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
        let generation = self
            .pending_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session)
            .unwrap_or(0);
        let admission_snapshot = match self
            .workspace_convergence
            .begin_admission(session, &facts.device_id, generation)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(
                    session = %session,
                    error = %error,
                    "begin_admission failed; rejecting with Internal"
                );
                self.handshake
                    .reject(
                        session,
                        PairingRejectReason::Internal(format!("begin_admission: {error}")),
                    )
                    .await;
                self.emit_failure(session, PairingFailureReason::Internal);
                return;
            }
        };

        let security_update_payload = match self
            .handshake
            .confirm(session, admission_snapshot.history_event_count as u64)
            .await
        {
            Ok(payload) => payload,
            Err(err) => {
                warn!(
                    session = %session,
                    error = %err,
                    "Confirm wire send failed after the admission record was saved"
                );
                // The in-flight admission record already landed — the owner
                // will re-await the same joiner's readiness after a restart.
                // `handshake.confirm` has already removed ctx + closed on the
                // happy path; on this Err path the coordinator did not close
                // (it short-circuited on the settings/send failure). We
                // deliberately do not send a Reject here because the joiner's
                // local store may have already advanced; let the natural
                // timeout take care of it.
                self.emit_failure(session, PairingFailureReason::ConnectionLost);
                return;
            }
        };
        let mut facts = facts;
        facts.security_update_payload = security_update_payload;
        self.pending_joiner_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session.clone(), facts);
    }

    async fn complete_after_joiner_ready(&self, session: &PairingSessionId, ready: JoinerReady) {
        let joiner_device_id = ready.admission.device_id.clone();
        let facts = self
            .pending_joiner_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session);
        let facts_valid = match &facts {
            Some(facts) => {
                ready.admission.device_id == facts.device_id
                    && ready.admission.device_name == facts.device_name
                    && ready.admission.identity_fingerprint == facts.identity_fingerprint
            }
            None => {
                // The sponsor restarted after `begin_admission` saved the
                // in-flight record: re-await the same joiner's readiness
                // from the persisted record instead of rejecting it. Only
                // the device id can be re-verified from that record.
                let Ok(Some(record)) = self.workspace_convergence.pending_admission(session).await
                else {
                    self.handshake
                        .reject(
                            session,
                            PairingRejectReason::Internal("unexpected joiner readiness".into()),
                        )
                        .await;
                    return;
                };
                if record.joiner_device_id != ready.admission.device_id {
                    self.handshake
                        .reject(
                            session,
                            PairingRejectReason::Internal("joiner readiness facts mismatch".into()),
                        )
                        .await;
                    self.emit_failure(session, PairingFailureReason::Internal);
                    return;
                }
                debug!(
                    session = %session,
                    joiner_device_id = %ready.admission.device_id.as_str(),
                    "restored in-flight admission record after sponsor restart"
                );
                true
            }
        };
        if !facts_valid {
            self.handshake
                .reject(
                    session,
                    PairingRejectReason::Internal("joiner readiness facts mismatch".into()),
                )
                .await;
            self.emit_failure(session, PairingFailureReason::Internal);
            return;
        }
        let security_update_payload = facts
            .as_ref()
            .map_or_else(Vec::new, |facts| facts.security_update_payload.clone());
        let committed = match self
            .workspace_convergence
            .commit_joiner_admission(session, ready.admission, security_update_payload)
            .await
        {
            Ok(committed) => committed,
            Err(WorkspaceConvergenceError::AdmissionGenerationAdvanced) => {
                self.handshake
                    .reject(session, PairingRejectReason::AdmissionUnavailable)
                    .await;
                self.emit_failure(session, PairingFailureReason::Internal);
                return;
            }
            Err(error) => {
                warn!(
                    session = %session,
                    error = %error,
                    "could not commit the joiner admission in workspace convergence"
                );
                self.handshake
                    .reject(
                        session,
                        PairingRejectReason::Internal(format!("commit_joiner_admission: {error}")),
                    )
                    .await;
                self.emit_failure(session, PairingFailureReason::Internal);
                return;
            }
        };
        if let Err(error) = self
            .handshake
            .send_committed(
                session,
                uc_core::pairing::SponsorAdmissionSaved { facts: committed },
            )
            .await
        {
            warn!(session = %session, error = %error, "could not send the admission-saved confirmation");
            // The change is already saved; the joiner stays locally ready
            // and recovers via the workspace handoff/restricted channel.
            self.handshake.complete(session).await;
            return;
        }
        self.handshake.complete(session).await;
        info!(
            session = %session,
            joiner_device_id = %joiner_device_id.as_str(),
            "joiner admission change saved and confirmed"
        );
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
        PairingSessionMessage::Confirm(_) => "Confirm",
        PairingSessionMessage::Ready(_) => "Ready",
        PairingSessionMessage::AdmissionSaved(_) => "AdmissionSaved",
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
    use uc_core::membership::{
        AdmissionChangeFacts, AdmissionSavedFacts, MemberInstanceId, MemberRepositoryPort,
        MembershipAdmissionDecision, MembershipError, SpaceMember, WorkspacePhase,
        WorkspaceSnapshot,
    };
    use uc_core::pairing::invitation::{InvitationCode, PairingInvitation};
    use uc_core::pairing::session_message::{
        JoinerChallengeResponse, JoinerReady, PairingReject, PairingSecurityCapability,
    };
    use uc_core::ports::pairing::{DialError, DialOutcome, PairingSessionPort, SessionError};
    use uc_core::ports::pairing_invitation::{InvitationError, IssuedInvitation};
    use uc_core::ports::space::{
        GroupAdmissionPort, PrepareAdmissionOfferPort, ProofPort, SpaceAccessError,
    };
    use uc_core::ports::{
        ClockPort, ConsumeInvitationError, DeviceIdentityPort, LocalIdentityError,
        LocalIdentityPort, PairingInvitationPort, SettingsPort, SetupStatusPort,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;
    use uc_core::space_access::domain::{
        GroupAdmission, PreparedAdmissionOffer, PreparedGroupJoin, ProofDerivedKey,
        SpaceAccessProofArtifact,
    };
    use uc_observability_contract::analytics::{
        AnalyticsFacade, AnalyticsPort, DefaultAnalyticsFacade, NoopAnalyticsIdentity,
    };

    use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
    use crate::space::admission::invitation::holder::InMemoryPairingInvitationHolder;
    use crate::space::convergence::WorkspaceConvergenceError;

    use crate::space::convergence::group_update_delivery::GroupUpdateDeliveryPort;

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
        fail_begin: bool,
        fail_commit: bool,
    }
    impl RecordingOwner {
        fn allowed() -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                decision: MembershipAdmissionDecision::Allowed,
                fail_begin: false,
                fail_commit: false,
            }
        }
        fn with_decision(decision: MembershipAdmissionDecision) -> Self {
            Self {
                decision,
                ..Self::allowed()
            }
        }
        fn with_fail_begin() -> Self {
            Self {
                fail_begin: true,
                ..Self::allowed()
            }
        }
        fn with_fail_commit() -> Self {
            Self {
                fail_commit: true,
                ..Self::allowed()
            }
        }
        fn call_log(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for RecordingOwner {
        async fn admission_decision_for_joiner(
            &self,
            _: u64,
            _: &DeviceId,
        ) -> MembershipAdmissionDecision {
            self.calls.lock().unwrap().push("admission_decision");
            self.decision
        }
        async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
            Ok(())
        }

        async fn begin_admission(
            &self,
            session: &PairingSessionId,
            joiner_device_id: &DeviceId,
            invitation_generation: u64,
        ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("begin_admission");
            assert_eq!(joiner_device_id.as_str(), "joiner-device");
            assert_eq!(invitation_generation, 0);
            assert_eq!(session.as_str(), "session-1");
            if self.fail_begin {
                return Err(WorkspaceConvergenceError::Unavailable);
            }
            Ok(owner_snapshot())
        }
        async fn commit_joiner_admission(
            &self,
            session: &PairingSessionId,
            joiner: AdmissionChangeFacts,
            _security_update_payload: Vec<u8>,
        ) -> Result<AdmissionSavedFacts, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("commit_joiner_admission");
            assert_eq!(session.as_str(), "session-1");
            assert_eq!(joiner.device_id.as_str(), "joiner-device");
            if self.fail_commit {
                return Err(WorkspaceConvergenceError::Unavailable);
            }
            Ok(AdmissionSavedFacts {
                history_digest: [0x11; 32],
                history_event_count: 2,
                sponsor_facts: joiner_facts(),
            })
        }
        async fn local_admission_facts(
            &self,
            _member_instance: Option<MemberInstanceId>,
        ) -> Result<AdmissionChangeFacts, WorkspaceConvergenceError> {
            unimplemented!("joiner-side method not exercised in sponsor tests")
        }
        async fn pending_admission(
            &self,
            _session: &uc_core::ports::pairing::PairingSessionId,
        ) -> Result<Option<uc_core::membership::PendingAdmissionRecord>, WorkspaceConvergenceError>
        {
            Ok(None)
        }
        async fn record_local_readiness(
            &self,
            _: MemberInstanceId,
        ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
            unimplemented!("joiner-side method not exercised in sponsor tests")
        }
        async fn record_admission_saved(
            &self,
            _: AdmissionSavedFacts,
        ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
            unimplemented!("joiner-side method not exercised in sponsor tests")
        }
    }

    fn owner_snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            phase: WorkspacePhase::LocallyApplied,
            revision: 1,
            history_event_count: 0,
            effective_member_count: 1,
            pending_removal_decision_device_ids: Vec::new(),
            pending_removal_decision_event_id: None,
            diverged_peer_device_ids: Vec::new(),
            upgrade_required_peer_device_ids: Vec::new(),
            convergence_digest: None,
            removed: false,
            updated_at_ms: fixed_now_ms(),
            failure_category: None,
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

        #[async_trait]
        impl GroupAdmissionPort for SpaceAccess {
            async fn prepare_group_join(
                &self,
                device_id: &DeviceId,
            ) -> Result<PreparedGroupJoin, SpaceAccessError>;
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
                passphrase: &uc_core::crypto::domain::Passphrase,
                pending: PreparedGroupJoin,
                welcome: &[u8],
                encrypted_key_catalog: &[u8],
                group_epoch: u64,
            ) -> Result<(), SpaceAccessError>;
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
        mock.expect_admit_group_member().returning(|_, _, _, _, _| {
            Ok(GroupAdmission {
                welcome: vec![1],
                encrypted_key_catalog: vec![2],
                existing_member_updates: Vec::new(),
                group_epoch: 2,
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

    struct NamedSettings(String);
    #[async_trait]
    impl SettingsPort for NamedSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            let mut s = Settings::default();
            s.general.device_name = Some(self.0.clone());
            Ok(s)
        }
        async fn save(&self, _: &Settings) -> anyhow::Result<()> {
            Ok(())
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

    struct NoopMemberRepo;
    #[async_trait]
    impl MemberRepositoryPort for NoopMemberRepo {
        async fn get(&self, _: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(None)
        }
        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }
        async fn save(&self, _: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }
        async fn remove(&self, _: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
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
    fn sponsor_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("BBBBBBBBBBBBBBBB").unwrap()
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
        JoinerRequest {
            invitation_code: InvitationCode::new(code),
            device_id: DeviceId::new("joiner-device"),
            device_name: "joiner's laptop".into(),
            identity_fingerprint: joiner_fp(),
            nonce: vec![1, 2, 3, 4],
            transport_address_blob: vec![],
            security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
            key_package: vec![1, 2, 3],
        }
    }

    fn joiner_facts() -> AdmissionChangeFacts {
        AdmissionChangeFacts {
            member_instance: MemberInstanceId::from_bytes([7; 32]),
            device_id: DeviceId::new("joiner-device"),
            device_name: "joiner's laptop".into(),
            identity_fingerprint: joiner_fp(),
            transport_public_key: vec![1; 32],
            transport_address_blob: vec![],
            identity_signature: vec![2; 64],
        }
    }

    fn joiner_ready() -> JoinerReady {
        JoinerReady {
            admission: joiner_facts(),
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
                space_access.clone(),
                space_access,
                noop_delivery(),
                Arc::new(NoopMemberRepo),
                Arc::new(ScriptedProof(StdMutex::new(self.proof_verdicts))),
                Arc::new(FixedLocal(sponsor_fp())),
                Arc::new(FixedDevice(DeviceId::new("sponsor-device"))),
                Arc::new(NamedSettings("sponsor-mac".into())),
                Arc::new(OrchestratorStubSetupStatus),
                Arc::new(uc_observability_contract::analytics::NoopAnalyticsFacade),
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
    async fn happy_path_saves_admission_then_confirms_and_commits() {
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
            message: PairingSessionMessage::Ready(joiner_ready()),
        })
        .await;

        let sent = session_port.sent();
        let kinds = sent
            .iter()
            .map(|(_, m)| variant_name(m))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["AdmissionOffer", "Confirm", "AdmissionSaved"],
            "wire sequence: offer → confirm → admission-saved confirmation"
        );
        assert!(
            matches!(
                sent.last().map(|(_, m)| m),
                Some(PairingSessionMessage::AdmissionSaved(_))
            ),
            "last wire frame must be the admission-saved confirmation"
        );
        assert_eq!(session_port.closed().len(), 1);
        // The owner saw: decision → begin → commit.
        let calls = owner.call_log();
        assert_eq!(
            calls,
            vec![
                "admission_decision",
                "begin_admission",
                "commit_joiner_admission"
            ]
        );
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
    async fn begin_admission_failure_rejects_with_internal() {
        let mut bundle = Bundle::happy();
        bundle.owner = Arc::new(RecordingOwner::with_fail_begin());
        bundle.holder.insert(pending("CODE-1")).await;
        let (orch, session_port, _owner) = bundle.build();
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
        let sent = session_port.sent();
        assert!(
            sent.iter().any(|(_, m)| matches!(
                m,
                PairingSessionMessage::Reject(PairingReject {
                    reason: PairingRejectReason::Internal(_),
                })
            )),
            "expected Internal reject after begin_admission failure, got {sent:?}"
        );
        assert!(
            !sent
                .iter()
                .any(|(_, m)| matches!(m, PairingSessionMessage::Confirm(_))),
            "no Confirm after a failed admission save"
        );
    }

    #[tokio::test]
    async fn commit_failure_rejects_and_never_sends_confirmation() {
        let mut bundle = Bundle::happy();
        bundle.owner = Arc::new(RecordingOwner::with_fail_commit());
        bundle.holder.insert(pending("CODE-1")).await;
        let (orch, session_port, _owner) = bundle.build();
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
            message: PairingSessionMessage::Ready(joiner_ready()),
        })
        .await;
        let sent = session_port.sent();
        assert!(
            !sent
                .iter()
                .any(|(_, m)| matches!(m, PairingSessionMessage::AdmissionSaved(_))),
            "no admission-saved confirmation after a failed commit"
        );
        assert!(
            sent.iter().any(|(_, m)| matches!(
                m,
                PairingSessionMessage::Reject(PairingReject {
                    reason: PairingRejectReason::Internal(_),
                })
            )),
            "expected Internal reject after commit failure"
        );
    }

    #[tokio::test]
    async fn ready_facts_mismatch_is_rejected_without_saving() {
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
        let mut mismatched = joiner_ready();
        mismatched.admission.device_id = DeviceId::new("other-device");
        orch.handle_event(PairingSessionEvent::MessageReceived {
            session: session.clone(),
            message: PairingSessionMessage::Ready(mismatched),
        })
        .await;
        assert_eq!(
            owner.call_log(),
            vec!["admission_decision", "begin_admission"],
            "a readiness mismatch must not reach commit_joiner_admission"
        );
        assert!(
            !session_port
                .sent()
                .iter()
                .any(|(_, m)| matches!(m, PairingSessionMessage::AdmissionSaved(_))),
            "no confirmation on a readiness mismatch"
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
            message: PairingSessionMessage::Ready(joiner_ready()),
        })
        .await;
        assert!(
            sink.events().is_empty(),
            "no analytics events after a committed admission (success is expressed by the workspace state)"
        );
    }
}
