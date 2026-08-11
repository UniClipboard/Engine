//! B2 · `RedeemPairingInvitationUseCase` (joiner side).
//!
//! Internal communication half of workspace admission (ADR-017): delegates
//! wire + crypto to [`JoinerHandshakeCoordinator`], then hands every save
//! boundary to the workspace owner:
//!
//! 1. the joiner's local readiness facts are saved by the owner
//!    (`record_local_readiness`) before the readiness reply is sent;
//! 2. the sponsor's "admission change saved" confirmation is recorded by
//!    the owner (`record_admission_committed`) once received, together with
//!    the sponsor's member facts.
//!
//! ## Ordering: owner saves before declaring success
//!
//! Mirrors sponsor-side ADR-017 cleanup: `record_local_readiness` →
//! `setup_status.set_status(completed)` land **before** the readiness
//! reply, and `record_admission_committed` lands before `execute` returns
//! `Ok`. The caller never gets a success result that isn't backed by fully
//! committed local state. Join success is not the result of the pairing
//! handshake: it is the fact that the workspace has saved the member
//! change, which the joiner only learns from the admission-saved
//! confirmation.
//!
//! `setup_status` is flipped after the readiness facts landed, because
//! `has_completed=true` is the marker `UnlockSpaceUseCase` keys off on the
//! next launch.
//!
//! ## Why no FSM
//!
//! See F-053: the joiner flow is linear once the passphrase is
//! collected up front (Slice 1 UX).
//!
//! [`JoinerHandshakeCoordinator`]:
//!     crate::space::admission::joiner::joiner_handshake::JoinerHandshakeCoordinator

use std::sync::Arc;

use tracing::{info, instrument};

use uc_core::ports::space::ResumeSpaceSessionPort;
use uc_core::ports::SetupStatusPort;
use uc_core::setup::SetupStatus;
use uc_observability_contract::analytics::events::{Event, PairingFailureReason, PairingMethod};
use uc_observability_contract::analytics::AnalyticsFacade;

use crate::facade::space_setup::commands::RedeemPairingInvitationCommand;
use crate::facade::space_setup::{RedeemPairingInvitationError, RedeemPairingInvitationResult};
use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
use crate::space::admission::joiner::joiner_handshake::{
    JoinerHandshakeCoordinator, JoinerHandshakeOutcome,
};

pub(crate) struct RedeemPairingInvitationUseCase {
    handshake: Arc<JoinerHandshakeCoordinator>,
    setup_status: Arc<dyn SetupStatusPort>,
    resume_session: Arc<dyn ResumeSpaceSessionPort>,
    /// Joiner-side analytics: fires `pairing_started` on entry,
    /// `pairing_failed` on failure. The success funnel no longer exists:
    /// join results are expressed through the workspace state.
    /// All calls are fire-and-forget; the gate inside the facade
    /// implementation keeps them off the hot path.
    analytics: Arc<dyn AnalyticsFacade>,
    /// The workspace owner behind the admission seam. Never `None`: the
    /// assembly layer guarantees the owner always exists.
    workspace_convergence: Arc<dyn WorkspaceAdmissionOwnerPort>,
}

impl RedeemPairingInvitationUseCase {
    pub(crate) fn new(
        handshake: Arc<JoinerHandshakeCoordinator>,
        setup_status: Arc<dyn SetupStatusPort>,
        resume_session: Arc<dyn ResumeSpaceSessionPort>,
        analytics: Arc<dyn AnalyticsFacade>,
        workspace_convergence: Arc<dyn WorkspaceAdmissionOwnerPort>,
    ) -> Self {
        Self {
            handshake,
            setup_status,
            resume_session,
            analytics,
            workspace_convergence,
        }
    }

    #[instrument(skip_all, fields(code = %cmd.code.as_str()))]
    pub(crate) async fn execute(
        &self,
        cmd: RedeemPairingInvitationCommand,
    ) -> Result<RedeemPairingInvitationResult, RedeemPairingInvitationError> {
        // Slice 8b · pairing_started 在 execute 入口立即 fire,即使 handshake
        // 第一行就拒绝(InvitationNotFound)也保证 funnel 第一步留下信号。
        self.analytics.capture(Event::PairingStarted {
            method: PairingMethod::Code,
        });
        let result = async {
            let pending = self.handshake.handshake(&cmd.code, &cmd.passphrase).await?;
            let channel = pending.outcome().discovery_channel;
            let persisted = match self.persist(pending.outcome().clone()).await {
                Ok(persisted) => persisted,
                Err(error) => {
                    self.handshake.abort(pending, &error.to_string()).await;
                    return Err(error);
                }
            };
            if self
                .resume_session
                .try_resume_session(&persisted.space_id)
                .await
                .map_err(|error| {
                    RedeemPairingInvitationError::Internal(format!(
                        "activate paired space: {error}"
                    ))
                })?
                .is_none()
            {
                self.handshake
                    .abort(pending, "paired space could not be activated")
                    .await;
                return Err(RedeemPairingInvitationError::Internal(
                    "activate paired space: persisted session was unavailable".into(),
                ));
            }
            // The joiner's local readiness facts (own member instance and
            // readiness record) are saved by the owner before the readiness
            // reply leaves this device. The member instance is the one
            // derived from this admission's fresh credential so a rejoining
            // device is never identified by a stale instance.
            let admission = self
                .workspace_convergence
                .local_admission_facts(pending.outcome().member_instance)
                .await
                .map_err(|error| {
                    RedeemPairingInvitationError::Internal(format!(
                        "prepare workspace admission: {error}"
                    ))
                })?;
            self.workspace_convergence
                .record_local_readiness(admission.member_instance)
                .await
                .map_err(|error| {
                    RedeemPairingInvitationError::Internal(format!("save local readiness: {error}"))
                })?;
            let committed = self.handshake.complete(pending, admission).await?;
            // The sponsor saved the admission change; the joiner records
            // the confirmation and the sponsor's member facts with the
            // owner before reporting success.
            self.workspace_convergence
                .record_admission_committed(committed.facts)
                .await
                .map_err(|error| {
                    RedeemPairingInvitationError::Internal(format!(
                        "record admission committed: {error}"
                    ))
                })?;
            Ok((persisted, channel))
        }
        .await;
        match &result {
            Ok(_) => {}
            Err(err) => self.analytics.capture(Event::PairingFailed {
                method: PairingMethod::Code,
                failure_reason: map_redeem_error_to_pairing_failure_reason(err),
            }),
        }
        result.map(|(res, _)| res)
    }

    /// Mark setup complete. Ordering rationale: see module doc — this runs
    /// after the owner saved the local readiness facts and before the
    /// readiness reply is sent.
    async fn persist(
        &self,
        outcome: JoinerHandshakeOutcome,
    ) -> Result<RedeemPairingInvitationResult, RedeemPairingInvitationError> {
        // Mark setup complete (ordering rationale: see module doc).
        self.setup_status
            .set_status(&SetupStatus {
                has_completed: true,
                space_id: Some(outcome.space_id.clone()),
            })
            .await
            .map_err(|e| {
                RedeemPairingInvitationError::Internal(format!("setup_status.set_status: {e}"))
            })?;

        info!(
            sponsor_device_id = %outcome.sponsor_device_id.as_str(),
            space_id = %outcome.space_id,
            "joiner local setup complete; space ready"
        );

        Ok(RedeemPairingInvitationResult {
            sponsor_device_id: outcome.sponsor_device_id,
            sponsor_identity_fingerprint: outcome.sponsor_identity_fingerprint,
            space_id: outcome.space_id,
            self_device_id: outcome.self_device_id,
            self_identity_fingerprint: outcome.self_identity_fingerprint,
        })
    }
}

/// Slice 8b · `RedeemPairingInvitationError` → `PairingFailureReason` 1:1
/// 映射。每个业务变体单独落到独立的 funnel 漏点信号,避免跨 domain 聚合
/// 时丢失"这条 join 是 passphrase 错 vs sponsor 主动拒绝 vs 网络超时"
/// 的关键区分。`Internal` / `SponsorInternal` 占比是架构债务指标
/// (schema doc §7.4)。
fn map_redeem_error_to_pairing_failure_reason(
    err: &RedeemPairingInvitationError,
) -> PairingFailureReason {
    match err {
        RedeemPairingInvitationError::InvitationNotFound => {
            PairingFailureReason::InvitationNotFound
        }
        RedeemPairingInvitationError::InvitationExpired => PairingFailureReason::InvitationExpired,
        RedeemPairingInvitationError::SponsorUnreachable => {
            PairingFailureReason::SponsorUnreachable
        }
        RedeemPairingInvitationError::ServiceUnavailable => {
            PairingFailureReason::ServiceUnavailable
        }
        RedeemPairingInvitationError::SponsorUpgradeRequired => {
            PairingFailureReason::SponsorUpgradeRequired
        }
        RedeemPairingInvitationError::PassphraseMismatch => {
            PairingFailureReason::PassphraseMismatch
        }
        RedeemPairingInvitationError::CorruptedKeyMaterial => {
            PairingFailureReason::CorruptedKeyMaterial
        }
        RedeemPairingInvitationError::DeviceNameRequired => {
            PairingFailureReason::DeviceNameRequired
        }
        RedeemPairingInvitationError::SponsorRejectedInvitation => {
            PairingFailureReason::SponsorRejectedInvitation
        }
        RedeemPairingInvitationError::SponsorAdmissionUnavailable => {
            PairingFailureReason::SponsorInternal
        }
        RedeemPairingInvitationError::SponsorDeclined => PairingFailureReason::SponsorDeclined,
        RedeemPairingInvitationError::SponsorTimedOut => PairingFailureReason::SponsorTimedOut,
        RedeemPairingInvitationError::SponsorInternal(_) => PairingFailureReason::SponsorInternal,
        RedeemPairingInvitationError::Timeout => PairingFailureReason::Timeout,
        RedeemPairingInvitationError::ConnectionLost => PairingFailureReason::ConnectionLost,
        RedeemPairingInvitationError::Internal(_) => PairingFailureReason::Internal,
    }
}

#[cfg(test)]
mod tests {
    //! Composition tests only: wire + crypto covered in
    //! [`crate::space::admission::joiner::joiner_handshake::tests`].
    //! Here we verify the joiner side of the admission seam (ADR-017):
    //! the workspace owner saves the local readiness facts before the
    //! readiness reply leaves the device, and the admission-saved
    //! confirmation is recorded before `execute` reports success. The
    //! channel side is verified against a workspace-owner double, so no
    //! real owner or real network is involved.
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use tokio::time::Duration;

    use uc_core::crypto::domain::{ActiveSpace, Passphrase};
    use uc_core::ids::{DeviceId, SessionId, SpaceId};
    use uc_core::membership::{
        AdmissionChangeFacts, AdmissionCommittedFacts, MemberInstanceId, RemovalAdmissionDecision,
        WorkspacePhase, WorkspaceSnapshot,
    };
    use uc_core::pairing::invitation::InvitationCode;
    use uc_core::pairing::session_message::{
        PairingReject, PairingRejectReason, PairingSessionMessage, SponsorAdmissionCommitted,
        SponsorAdmissionOffer, SponsorConfirm,
    };
    use uc_core::ports::pairing::{
        DialError, DialOutcome, DiscoveryChannel, PairingSessionId, PairingSessionPort,
        SessionError,
    };
    use uc_core::ports::space::{
        DeriveAdmissionProofKeyPort, GroupAdmissionPort, ProofPort, ResumeSpaceSessionPort,
        SpaceAccessError,
    };
    use uc_core::ports::{
        DeviceIdentityPort, LocalIdentityError, LocalIdentityPort, SettingsPort, SetupStatusPort,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;
    use uc_core::setup::SetupStatus;
    use uc_core::space_access::domain::{
        AdmissionOffer, GroupAdmission, PreparedGroupJoin, ProofDerivedKey,
        SpaceAccessProofArtifact,
    };

    use crate::space::convergence::WorkspaceConvergenceError;

    // ── wire fakes (produce a happy-path outcome) ─────────────────────────

    #[derive(Default)]
    struct HappySession {
        sent: StdMutex<Vec<PairingSessionMessage>>,
        recv: StdMutex<VecDeque<PairingSessionMessage>>,
        closed: StdMutex<u32>,
    }
    impl HappySession {
        fn primed() -> Self {
            let me = Self::default();
            me.push_offer_and_confirm();
            me.recv
                .lock()
                .unwrap()
                .push_back(PairingSessionMessage::AdmissionCommitted(
                    SponsorAdmissionCommitted {
                        facts: committed_facts(),
                    },
                ));
            me
        }

        fn push_offer_and_confirm(&self) {
            self.recv
                .lock()
                .unwrap()
                .push_back(PairingSessionMessage::AdmissionOffer(
                    SponsorAdmissionOffer {
                        space_id: SpaceId::from_str("space-xyz"),
                        kdf_parameters_blob: vec![0xAA; 16],
                        challenge: vec![0x42; 32],
                        pairing_session_id: PairingSessionId::new("session-1"),
                    },
                ));
            self.recv
                .lock()
                .unwrap()
                .push_back(PairingSessionMessage::Confirm(SponsorConfirm {
                    space_id: SpaceId::from_str("space-xyz"),
                    sender_device_id: DeviceId::new("sponsor-device"),
                    sender_device_name: "sponsor's laptop".into(),
                    sender_identity_fingerprint: sponsor_fp(),
                    transport_address_blob: Vec::new(),
                    sponsor_space_person_id: None,
                    welcome: vec![1],
                    encrypted_key_catalog: vec![2],
                    group_epoch: 2,
                }));
        }
    }
    #[async_trait]
    impl PairingSessionPort for HappySession {
        async fn dial_by_invitation(&self, _: &InvitationCode) -> Result<DialOutcome, DialError> {
            Ok(DialOutcome {
                session_id: PairingSessionId::new("session-1"),
                channel: DiscoveryChannel::Cloud,
            })
        }
        async fn send(
            &self,
            _: &PairingSessionId,
            m: PairingSessionMessage,
        ) -> Result<(), SessionError> {
            self.sent.lock().unwrap().push(m);
            Ok(())
        }
        async fn recv_next(
            &self,
            _: &PairingSessionId,
        ) -> Result<Option<PairingSessionMessage>, SessionError> {
            Ok(self.recv.lock().unwrap().pop_front())
        }
        async fn close(&self, _: &PairingSessionId, _: Option<String>) {
            *self.closed.lock().unwrap() += 1;
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
                passphrase: &Passphrase,
                pending: PreparedGroupJoin,
                welcome: &[u8],
                encrypted_key_catalog: &[u8],
                group_epoch: u64,
            ) -> Result<(), SpaceAccessError>;
        }
    }

    fn happy_space_access() -> Arc<MockSpaceAccess> {
        let mut mock = MockSpaceAccess::new();
        mock.expect_prepare_group_join()
            .returning(|_| Ok(PreparedGroupJoin::new(vec![1, 2, 3], vec![4, 5, 6])));
        mock.expect_derive_admission_proof_key()
            .returning(|_, _, _, _| Ok(ProofDerivedKey::from_bytes([0xCC; 32])));
        mock.expect_install_group_join()
            .returning(|_, _, _, _, _, _| Ok(()));
        Arc::new(mock)
    }

    struct FixedProof;
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
                proof_bytes: vec![0xFE; 32],
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

    struct RecordingSetupStatus {
        fail_next: StdMutex<bool>,
        set_calls: StdMutex<Vec<bool>>,
    }
    impl RecordingSetupStatus {
        fn ok() -> Self {
            Self {
                fail_next: StdMutex::new(false),
                set_calls: StdMutex::new(Vec::new()),
            }
        }
        fn failing() -> Self {
            Self {
                fail_next: StdMutex::new(true),
                set_calls: StdMutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl SetupStatusPort for RecordingSetupStatus {
        async fn get_status(&self) -> anyhow::Result<SetupStatus> {
            Ok(SetupStatus::default())
        }
        async fn set_status(&self, s: &SetupStatus) -> anyhow::Result<()> {
            if *self.fail_next.lock().unwrap() {
                return Err(anyhow::anyhow!("setup-status backend down"));
            }
            self.set_calls.lock().unwrap().push(s.has_completed);
            Ok(())
        }
    }

    struct ReadyResume;
    #[async_trait]
    impl ResumeSpaceSessionPort for ReadyResume {
        async fn try_resume_session(
            &self,
            space_id: &SpaceId,
        ) -> Result<Option<ActiveSpace>, SpaceAccessError> {
            Ok(Some(ActiveSpace::new(space_id.clone())))
        }
    }

    // ── workspace-owner double (the admission seam's channel-side test) ──

    #[derive(Default)]
    struct RecordingOwner {
        calls: StdMutex<Vec<&'static str>>,
        fail_readiness: StdMutex<bool>,
        fail_committed: StdMutex<bool>,
        committed_facts: StdMutex<Option<AdmissionCommittedFacts>>,
    }
    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for RecordingOwner {
        async fn admission_decision(&self, _: u64) -> RemovalAdmissionDecision {
            RemovalAdmissionDecision::Allowed
        }
        async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
            Ok(())
        }

        async fn begin_admission(
            &self,
            _: &PairingSessionId,
            _: &DeviceId,
            _: u64,
        ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
            unimplemented!("sponsor-side method not exercised in joiner tests")
        }
        async fn commit_joiner_admission(
            &self,
            _: &PairingSessionId,
            _: AdmissionChangeFacts,
            _security_update_payload: Vec<u8>,
        ) -> Result<AdmissionCommittedFacts, WorkspaceConvergenceError> {
            unimplemented!("sponsor-side method not exercised in joiner tests")
        }
        async fn local_admission_facts(
            &self,
            _member_instance: Option<MemberInstanceId>,
        ) -> Result<AdmissionChangeFacts, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("local_admission_facts");
            Ok(joiner_facts())
        }
        async fn record_local_readiness(
            &self,
            own_instance: MemberInstanceId,
        ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("record_local_readiness");
            if *self.fail_readiness.lock().unwrap() {
                return Err(WorkspaceConvergenceError::Unavailable);
            }
            assert_eq!(own_instance, MemberInstanceId::from_bytes([7; 32]));
            Ok(snapshot())
        }
        async fn record_admission_committed(
            &self,
            confirmation: AdmissionCommittedFacts,
        ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
            self.calls
                .lock()
                .unwrap()
                .push("record_admission_committed");
            if *self.fail_committed.lock().unwrap() {
                return Err(WorkspaceConvergenceError::Unavailable);
            }
            *self.committed_facts.lock().unwrap() = Some(confirmation);
            Ok(snapshot())
        }
        async fn pending_admission(
            &self,
            _session: &uc_core::ports::pairing::PairingSessionId,
        ) -> Result<Option<uc_core::membership::PendingAdmissionRecord>, WorkspaceConvergenceError>
        {
            Ok(None)
        }
    }

    fn committed_facts() -> AdmissionCommittedFacts {
        AdmissionCommittedFacts {
            change_digest: [0x11; 32],
            change_count: 2,
            sponsor_facts: AdmissionChangeFacts {
                member_instance: MemberInstanceId::from_bytes([9; 32]),
                device_id: DeviceId::new("sponsor-device"),
                device_name: "sponsor's laptop".into(),
                identity_fingerprint: sponsor_fp(),
                transport_public_key: vec![3; 32],
                transport_address_blob: Vec::new(),
                identity_signature: vec![4; 64],
            },
        }
    }

    fn joiner_facts() -> AdmissionChangeFacts {
        AdmissionChangeFacts {
            member_instance: MemberInstanceId::from_bytes([7; 32]),
            device_id: DeviceId::new("joiner-device"),
            device_name: "joiner-laptop".into(),
            identity_fingerprint: joiner_fp(),
            transport_public_key: vec![5; 32],
            transport_address_blob: Vec::new(),
            identity_signature: vec![6; 64],
        }
    }

    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            phase: WorkspacePhase::LocallyApplied,
            revision: 1,
            change_count: 0,
            removal_intent_count: 0,
            effective_member_count: 1,
            confirmed_member_count: 0,
            waiting_member_device_ids: Vec::new(),
            waiting_member_count: 0,
            convergence_digest: None,
            removed: false,
            updated_at_ms: fixed_now_ms(),
            failure_category: None,
        }
    }

    // ── fixtures ─────────────────────────────────────────────────────────

    fn sponsor_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("BBBBBBBBBBBBBBBB").unwrap()
    }
    fn joiner_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("AAAAAAAAAAAAAAAA").unwrap()
    }
    fn fixed_now_ms() -> i64 {
        1775119200000
    }
    fn cmd(code: &str) -> RedeemPairingInvitationCommand {
        RedeemPairingInvitationCommand {
            code: InvitationCode::new(code),
            passphrase: Passphrase::new("hunter22hunter22"),
        }
    }

    #[derive(Default)]
    struct CapturingAnalyticsSink {
        events: StdMutex<Vec<Event>>,
    }
    impl uc_observability_contract::analytics::AnalyticsPort for CapturingAnalyticsSink {
        fn capture(&self, event: Event) {
            self.events.lock().unwrap().push(event);
        }
    }
    impl CapturingAnalyticsSink {
        fn snapshot(&self) -> Vec<Event> {
            self.events.lock().unwrap().clone()
        }
    }

    struct Harness {
        session: Arc<HappySession>,
        owner: Arc<RecordingOwner>,
        setup_status: Arc<RecordingSetupStatus>,
        analytics: Arc<CapturingAnalyticsSink>,
    }

    impl Harness {
        fn build(
            session: Arc<HappySession>,
            owner: Arc<RecordingOwner>,
            setup_status: Arc<RecordingSetupStatus>,
        ) -> (RedeemPairingInvitationUseCase, Self) {
            let space_access = happy_space_access();
            let handshake = JoinerHandshakeCoordinator::new(
                session.clone(),
                space_access.clone(),
                space_access,
                Arc::new(FixedProof),
                Arc::new(FixedLocal(joiner_fp())),
                Arc::new(FixedDevice(DeviceId::new("joiner-device"))),
                Arc::new(NamedSettings("joiner-laptop".into())),
                Duration::from_secs(30),
            );
            let analytics = Arc::new(CapturingAnalyticsSink::default());
            let facade: Arc<dyn AnalyticsFacade> = Arc::new(
                uc_observability_contract::analytics::DefaultAnalyticsFacade::new(
                    Arc::clone(&analytics)
                        as Arc<dyn uc_observability_contract::analytics::AnalyticsPort>,
                    Arc::new(uc_observability_contract::analytics::NoopAnalyticsIdentity),
                ),
            );
            let uc = RedeemPairingInvitationUseCase::new(
                handshake,
                setup_status.clone(),
                Arc::new(ReadyResume),
                facade,
                owner.clone() as Arc<dyn WorkspaceAdmissionOwnerPort>,
            );
            (
                uc,
                Self {
                    session,
                    owner,
                    setup_status,
                    analytics,
                },
            )
        }

        fn happy() -> (RedeemPairingInvitationUseCase, Self) {
            Self::build(
                Arc::new(HappySession::primed()),
                Arc::new(RecordingOwner::default()),
                Arc::new(RecordingSetupStatus::ok()),
            )
        }
    }

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn happy_path_saves_readiness_before_ready_and_committed_before_return() {
        let (uc, h) = Harness::happy();
        let out = uc.execute(cmd("CODE-1")).await.unwrap();
        assert_eq!(out.sponsor_device_id.as_str(), "sponsor-device");
        assert_eq!(out.sponsor_identity_fingerprint, sponsor_fp());
        assert_eq!(out.space_id.inner(), "space-xyz");
        assert_eq!(out.self_device_id.as_str(), "joiner-device");
        assert_eq!(out.self_identity_fingerprint, joiner_fp());

        // 顺序：本机就绪事实由负责人保存 → 就绪回复发出 → 确认由负责人记录。
        let calls = h.owner.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                "local_admission_facts",
                "record_local_readiness",
                "record_admission_committed",
            ]
        );
        let sent = h.session.sent.lock().unwrap().clone();
        let ready_index = sent
            .iter()
            .position(|m| matches!(m, PairingSessionMessage::Ready(_)))
            .expect("readiness reply must be sent");
        assert_eq!(
            calls.iter().position(|c| *c == "record_local_readiness"),
            Some(0).and(Some(1)),
            "readiness saved before the reply leaves"
        );
        assert!(
            calls
                .iter()
                .position(|c| *c == "record_local_readiness")
                .unwrap()
                < ready_index,
            "record_local_readiness must precede the Ready frame"
        );
        assert_eq!(
            *h.setup_status.set_calls.lock().unwrap(),
            vec![true],
            "setup_status flipped exactly once to has_completed=true"
        );
        // 成功不再产生配对成功分析事件；只有开始事件。
        let events = h.analytics.snapshot();
        assert_eq!(events.len(), 1, "expected [PairingStarted], got {events:?}");
        assert!(matches!(events[0], Event::PairingStarted { .. }));
    }

    #[tokio::test]
    async fn readiness_save_failure_aborts_without_sending_ready() {
        let owner = Arc::new(RecordingOwner::default());
        *owner.fail_readiness.lock().unwrap() = true;
        let (uc, h) = Harness::build(
            Arc::new(HappySession::primed()),
            owner,
            Arc::new(RecordingSetupStatus::ok()),
        );
        let err = uc.execute(cmd("X")).await.unwrap_err();
        match err {
            RedeemPairingInvitationError::Internal(m) => {
                assert!(m.contains("save local readiness"), "msg = {m}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }
        let sent = h.session.sent.lock().unwrap().clone();
        assert!(
            !sent
                .iter()
                .any(|m| matches!(m, PairingSessionMessage::Ready(_))),
            "no readiness reply after a failed readiness save"
        );
        let events = h.analytics.snapshot();
        assert_eq!(events.len(), 2, "expected [PairingStarted, PairingFailed]");
        assert!(matches!(events[1], Event::PairingFailed { .. }));
    }

    #[tokio::test]
    async fn committed_record_failure_surfaces_internal() {
        let owner = Arc::new(RecordingOwner::default());
        *owner.fail_committed.lock().unwrap() = true;
        let (uc, _h) = Harness::build(
            Arc::new(HappySession::primed()),
            owner,
            Arc::new(RecordingSetupStatus::ok()),
        );
        let err = uc.execute(cmd("X")).await.unwrap_err();
        match err {
            RedeemPairingInvitationError::Internal(m) => {
                assert!(m.contains("record admission committed"), "msg = {m}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sponsor_reject_after_ready_surfaces_reject() {
        let session = Arc::new(HappySession::default());
        session.push_offer_and_confirm();
        session
            .recv
            .lock()
            .unwrap()
            .push_back(PairingSessionMessage::Reject(PairingReject {
                reason: PairingRejectReason::AdmissionUnavailable,
            }));
        let (uc, _h) = Harness::build(
            session,
            Arc::new(RecordingOwner::default()),
            Arc::new(RecordingSetupStatus::ok()),
        );
        let err = uc.execute(cmd("X")).await.unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::SponsorAdmissionUnavailable
        ));
    }

    #[tokio::test]
    async fn setup_status_failure_surfaces_internal() {
        let (uc, h) = Harness::build(
            Arc::new(HappySession::primed()),
            Arc::new(RecordingOwner::default()),
            Arc::new(RecordingSetupStatus::failing()),
        );
        let err = uc.execute(cmd("X")).await.unwrap_err();
        match err {
            RedeemPairingInvitationError::Internal(m) => {
                assert!(m.contains("setup_status.set_status"), "msg = {m}")
            }
            other => panic!("expected Internal, got {other:?}"),
        }
        assert!(h.setup_status.set_calls.lock().unwrap().is_empty());
        let events = h.analytics.snapshot();
        assert_eq!(events.len(), 2, "expected [PairingStarted, PairingFailed]");
    }

    /// 锁死 `RedeemPairingInvitationError` → `PairingFailureReason`
    /// 全变体的 1:1 映射。新增错误变体而忘了加映射时，这条会编译失败。
    #[test]
    fn map_redeem_error_covers_all_variants() {
        use super::map_redeem_error_to_pairing_failure_reason as map;
        use PairingFailureReason as R;
        use RedeemPairingInvitationError as E;
        let cases: Vec<(E, R)> = vec![
            (E::InvitationNotFound, R::InvitationNotFound),
            (E::InvitationExpired, R::InvitationExpired),
            (E::SponsorUnreachable, R::SponsorUnreachable),
            (E::ServiceUnavailable, R::ServiceUnavailable),
            (E::SponsorUpgradeRequired, R::SponsorUpgradeRequired),
            (E::PassphraseMismatch, R::PassphraseMismatch),
            (E::CorruptedKeyMaterial, R::CorruptedKeyMaterial),
            (E::DeviceNameRequired, R::DeviceNameRequired),
            (E::SponsorRejectedInvitation, R::SponsorRejectedInvitation),
            (E::SponsorDeclined, R::SponsorDeclined),
            (E::SponsorTimedOut, R::SponsorTimedOut),
            (E::SponsorInternal("boom".into()), R::SponsorInternal),
            (E::Timeout, R::Timeout),
            (E::ConnectionLost, R::ConnectionLost),
            (E::Internal("boom".into()), R::Internal),
        ];
        for (err, expected) in cases.iter() {
            assert_eq!(map(err), *expected, "for {err:?}");
        }
    }
}
