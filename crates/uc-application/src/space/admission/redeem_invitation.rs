//! B2 · `RedeemPairingInvitationUseCase` (joiner side).
//!
//! Internal communication half of workspace admission (ADR-017): delegates
//! wire + crypto to [`JoinerHandshakeCoordinator`]. Every durable admission
//! boundary is owned and saved by workspace convergence before its message is
//! sent.
//!
//! ## Ordering: owner saves before declaring success
//!
//! The caller never gets a result that is not backed by the saved workspace
//! state. Active joins mark setup complete and resume the target session;
//! Cross-Space joins remain Pending until the Engine drains the source
//! session and resumes the saved transition.
//!
//! current-Space and re-pairing state change only after local activation, because
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

use uc_observability_contract::analytics::events::{Event, PairingFailureReason, PairingMethod};
use uc_observability_contract::analytics::AnalyticsFacade;

use crate::facade::space_setup::commands::RedeemPairingInvitationCommand;
use crate::facade::space_setup::{RedeemPairingInvitationError, RedeemPairingInvitationResult};
use crate::space::admission::joiner::joiner_handshake::{
    JoinerHandshakeCoordinator, JoinerHandshakeOutcome,
};
use crate::space::re_pairing::RePairingState;
use crate::space::session::ResumeSpaceSessionPort;

pub(crate) struct RedeemPairingInvitationUseCase {
    handshake: Arc<JoinerHandshakeCoordinator>,
    resume_session: Arc<dyn ResumeSpaceSessionPort>,
    re_pairing_state: Arc<RePairingState>,
    /// Joiner-side analytics: fires `pairing_started` on entry,
    /// `pairing_failed` on failure. The success funnel no longer exists:
    /// join results are expressed through the workspace state.
    /// All calls are fire-and-forget; the gate inside the facade
    /// implementation keeps them off the hot path.
    analytics: Arc<dyn AnalyticsFacade>,
}

impl RedeemPairingInvitationUseCase {
    pub(crate) fn new(
        handshake: Arc<JoinerHandshakeCoordinator>,
        resume_session: Arc<dyn ResumeSpaceSessionPort>,
        re_pairing_state: Arc<RePairingState>,
        analytics: Arc<dyn AnalyticsFacade>,
    ) -> Self {
        Self {
            handshake,
            resume_session,
            re_pairing_state,
            analytics,
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
            let pending = self
                .handshake
                .handshake_with_history_policy(
                    &cmd.code,
                    &cmd.passphrase,
                    cmd.preserve_unreadable_history,
                )
                .await?;
            let channel = pending.outcome().discovery_channel;
            let requires_session_transition = pending.requires_session_transition();
            let persisted = match self
                .persist(pending.outcome().clone(), requires_session_transition)
                .await
            {
                Ok(persisted) => persisted,
                Err(error) => {
                    self.handshake.abort(pending, &error.to_string()).await;
                    return Err(error);
                }
            };
            if !requires_session_transition
                && self
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
        requires_session_transition: bool,
    ) -> Result<RedeemPairingInvitationResult, RedeemPairingInvitationError> {
        if !requires_session_transition {
            self.re_pairing_state
                .resolve_after_successful_pairing()
                .await
                .map_err(|error| {
                    RedeemPairingInvitationError::Internal(format!(
                        "re-pairing state update failed: {error}"
                    ))
                })?;
        }

        info!(
            sponsor_device_id = %outcome.sponsor_device_id.as_str(),
            space_id = %outcome.space_id,
            requires_session_transition,
            "joiner admission result persisted"
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
        RedeemPairingInvitationError::UnreadableHistoryRequiresConfirmation => {
            PairingFailureReason::Internal
        }
        RedeemPairingInvitationError::PreviousJoinCannotBeSuperseded => {
            PairingFailureReason::PreviousJoinCannotBeSuperseded
        }
        RedeemPairingInvitationError::SponsorRejectedInvitation => {
            PairingFailureReason::SponsorRejectedInvitation
        }
        RedeemPairingInvitationError::SponsorAdmissionUnavailable => {
            PairingFailureReason::SponsorInternal
        }
        RedeemPairingInvitationError::SponsorAdmissionConflict => {
            PairingFailureReason::SponsorAdmissionConflict
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
        AdmissionChangeFacts, MemberInstanceId, MembershipAdmissionDecision,
    };
    use uc_core::pairing::invitation::InvitationCode;
    use uc_core::pairing::session_message::{
        PairingReject, PairingRejectReason, PairingSessionMessage, SponsorAdmissionOffer,
    };
    use uc_core::ports::pairing::{
        DialError, DialOutcome, DiscoveryChannel, PairingSessionId, PairingSessionPort,
        SessionError,
    };
    use uc_core::ports::space::{
        DeriveAdmissionProofKeyPort, GroupAdmissionPort, ProofPort, SpaceAccessError,
    };
    use uc_core::ports::{DeviceIdentityPort, LocalIdentityError, LocalIdentityPort, SettingsPort};
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;
    use uc_core::space_access::domain::{
        AdmissionOffer, GroupAdmission, PreparedGroupJoin, ProofDerivedKey,
        SpaceAccessProofArtifact,
    };

    use crate::space::admission::adapter::WorkspaceAdmissionOwnerPort;
    use crate::space::session::ResumeSpaceSessionPort;
    use crate::space::workspace_membership::WorkspaceConvergenceError;

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
            me.push_offer_and_candidate();
            me.recv
                .lock()
                .unwrap()
                .push_back(PairingSessionMessage::DurableAdmission(durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::Commit,
                    Vec::new(),
                )));
            me.recv
                .lock()
                .unwrap()
                .push_back(PairingSessionMessage::DurableAdmission(durable_frame(
                    uc_core::pairing::DurableAdmissionMessageKind::Complete,
                    Vec::new(),
                )));
            me
        }

        fn push_offer_and_candidate(&self) {
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
                .push_back(PairingSessionMessage::DurableAdmission(candidate_frame()));
        }
    }
    #[async_trait]
    impl PairingSessionPort for HappySession {
        async fn dial_by_invitation(&self, _: &InvitationCode) -> Result<DialOutcome, DialError> {
            Ok(DialOutcome {
                session_id: PairingSessionId::new("session-1"),
                channel: DiscoveryChannel::Cloud,
                continuation_address: b"sponsor-address".to_vec(),
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

        async fn local_transport_public_key(&self) -> Option<Vec<u8>> {
            Some(vec![5; 32])
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
        impl uc_core::ports::space::PrepareAdmissionTargetAccessPort for SpaceAccess {
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

    fn happy_space_access() -> Arc<MockSpaceAccess> {
        let mut mock = MockSpaceAccess::new();
        mock.expect_prepare_group_join().returning(|device_id| {
            Ok(PreparedGroupJoin::new(vec![1, 2, 3], vec![4, 5, 6])
                .with_member_instance(joiner_credential().member_instance_id(device_id)))
        });
        mock.expect_prepared_join_membership_credential()
            .returning(|_| Ok(joiner_credential()));
        mock.expect_sign_prepared_join_payload()
            .returning(|_, _| Ok(vec![6; 64]));
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
        requires_session_transition: bool,
    }
    #[async_trait]
    impl WorkspaceAdmissionOwnerPort for RecordingOwner {
        async fn prepare_local_join_before_network(
            &self,
            preparation: &(dyn GroupAdmissionPort + Send + Sync),
            local_device_id: &DeviceId,
            _sponsor: &[u8],
            _sponsor_continuation_address: &[u8],
            _stable_request_binding: &[u8],
            _preserve_unreadable_history: bool,
        ) -> Result<
            crate::space::admission::adapter::DurableLocalJoinPreparation,
            WorkspaceConvergenceError,
        > {
            let prepared_group_join = preparation
                .prepare_group_join(local_device_id)
                .await
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
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

        async fn admission_decision_for_joiner(
            &self,
            _: u64,
            _: &DeviceId,
        ) -> MembershipAdmissionDecision {
            MembershipAdmissionDecision::Allowed
        }
        async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
            Ok(())
        }

        async fn prepare_joiner_candidate(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
            _proof_signer: &(dyn GroupAdmissionPort + Send + Sync),
            _target_access: &(dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort
                  + Send
                  + Sync),
            _passphrase: &Passphrase,
        ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("prepare_joiner_candidate");
            Ok(durable_frame(
                uc_core::pairing::DurableAdmissionMessageKind::Prepared,
                Vec::new(),
            ))
        }

        async fn apply_joiner_commit(
            &self,
            _frame: &uc_core::pairing::DurableAdmissionFrame,
            _receipt_signer: &(dyn GroupAdmissionPort + Send + Sync),
        ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
            self.calls.lock().unwrap().push("apply_joiner_commit");
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
            WorkspaceConvergenceError,
        > {
            self.calls.lock().unwrap().push("activate_joiner_complete");
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
            MembershipCredential, MembershipEventV2, MembershipOperationV2,
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
                member: MemberInstanceId::from_bytes([0x42; 32]),
            },
            [0x43; 32],
            [0x44; 32],
            Vec::new(),
            None,
            vec![0x45; 64],
        );
        let candidate = crate::space::admission::durable::DurableAdmissionCandidateV1 {
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
        let payload = crate::space::admission::durable::DurableAdmissionCandidatePayloadV1::new(
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

    // ── fixtures ─────────────────────────────────────────────────────────

    fn sponsor_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("BBBBBBBBBBBBBBBB").unwrap()
    }
    fn joiner_fp() -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string("AAAAAAAAAAAAAAAA").unwrap()
    }
    fn joiner_credential() -> uc_core::membership::MembershipCredential {
        uc_core::membership::MembershipCredential::new(1, vec![0x72; 32])
    }
    fn cmd(code: &str) -> RedeemPairingInvitationCommand {
        RedeemPairingInvitationCommand {
            code: InvitationCode::new(code),
            passphrase: Passphrase::new("hunter22hunter22"),
            preserve_unreadable_history: false,
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

    #[derive(Default)]
    struct InMemoryRePairingStateStore {
        required: Mutex<bool>,
    }

    #[async_trait]
    impl crate::deps::RePairingStateStorePort for InMemoryRePairingStateStore {
        async fn is_required(&self) -> Result<bool, crate::deps::RePairingStateError> {
            Ok(*self.required.lock().unwrap())
        }

        async fn set_required(
            &self,
            required: bool,
        ) -> Result<(), crate::deps::RePairingStateError> {
            *self.required.lock().unwrap() = required;
            Ok(())
        }
    }

    struct Harness {
        session: Arc<HappySession>,
        owner: Arc<RecordingOwner>,
        re_pairing_store: Arc<InMemoryRePairingStateStore>,
        analytics: Arc<CapturingAnalyticsSink>,
    }

    impl Harness {
        fn build(
            session: Arc<HappySession>,
            owner: Arc<RecordingOwner>,
        ) -> (RedeemPairingInvitationUseCase, Self) {
            let space_access = happy_space_access();
            let handshake = JoinerHandshakeCoordinator::new(
                session.clone(),
                space_access.clone(),
                space_access.clone(),
                space_access,
                Arc::new(FixedProof),
                Arc::new(FixedLocal(joiner_fp())),
                Arc::new(FixedDevice(DeviceId::new("joiner-device"))),
                Arc::new(NamedSettings("joiner-laptop".into())),
                owner.clone() as Arc<dyn WorkspaceAdmissionOwnerPort>,
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
            let re_pairing_store = Arc::new(InMemoryRePairingStateStore::default());
            *re_pairing_store.required.lock().unwrap() = true;
            let uc = RedeemPairingInvitationUseCase::new(
                handshake,
                Arc::new(ReadyResume),
                Arc::new(crate::space::re_pairing::RePairingState::new(
                    re_pairing_store.clone(),
                )),
                facade,
            );
            (
                uc,
                Self {
                    session,
                    owner,
                    re_pairing_store,
                    analytics,
                },
            )
        }

        fn happy() -> (RedeemPairingInvitationUseCase, Self) {
            Self::build(
                Arc::new(HappySession::primed()),
                Arc::new(RecordingOwner::default()),
            )
        }
    }

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn active_join_persists_setup_after_durable_completion() {
        let (uc, h) = Harness::happy();
        let out = uc.execute(cmd("CODE-1")).await.unwrap();
        assert_eq!(out.sponsor_device_id.as_str(), "sponsor-device");
        assert_eq!(out.sponsor_identity_fingerprint, sponsor_fp());
        assert_eq!(out.space_id.inner(), "space-xyz");
        assert_eq!(out.self_device_id.as_str(), "joiner-device");
        assert_eq!(out.self_identity_fingerprint, joiner_fp());

        let calls = h.owner.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                "prepare_joiner_candidate",
                "apply_joiner_commit",
                "activate_joiner_complete",
            ]
        );
        assert!(!*h.re_pairing_store.required.lock().unwrap());
        let sent = h.session.sent.lock().unwrap().clone();
        assert!(sent.iter().any(|message| matches!(
            message,
            PairingSessionMessage::DurableAdmission(frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::CompleteAck
        )));
        // 成功不再产生配对成功分析事件；只有开始事件。
        let events = h.analytics.snapshot();
        assert_eq!(events.len(), 1, "expected [PairingStarted], got {events:?}");
        assert!(matches!(events[0], Event::PairingStarted { .. }));
    }

    #[tokio::test]
    async fn cross_space_completion_stays_pending_without_changing_setup() {
        let owner = Arc::new(RecordingOwner {
            requires_session_transition: true,
            ..Default::default()
        });
        let (uc, h) = Harness::build(Arc::new(HappySession::primed()), owner);
        let result = uc.execute(cmd("X")).await.unwrap();
        assert_eq!(result.space_id.inner(), "space-xyz");
        assert!(*h.re_pairing_store.required.lock().unwrap());
        let sent = h.session.sent.lock().unwrap().clone();
        assert!(sent.iter().all(|message| !matches!(
            message,
            PairingSessionMessage::DurableAdmission(frame)
                if frame.kind == uc_core::pairing::DurableAdmissionMessageKind::CompleteAck
        )));
        let events = h.analytics.snapshot();
        assert_eq!(events.len(), 1, "pending is not a pairing failure");
    }

    #[tokio::test]
    async fn sponsor_reject_after_candidate_surfaces_reject() {
        let session = Arc::new(HappySession::default());
        session.push_offer_and_candidate();
        session
            .recv
            .lock()
            .unwrap()
            .push_back(PairingSessionMessage::Reject(PairingReject {
                reason: PairingRejectReason::AdmissionUnavailable,
            }));
        let (uc, _h) = Harness::build(session, Arc::new(RecordingOwner::default()));
        let err = uc.execute(cmd("X")).await.unwrap_err();
        assert!(matches!(
            err,
            RedeemPairingInvitationError::SponsorAdmissionUnavailable
        ));
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
            (E::UnreadableHistoryRequiresConfirmation, R::Internal),
            (
                E::PreviousJoinCannotBeSuperseded,
                R::PreviousJoinCannotBeSuperseded,
            ),
            (E::SponsorRejectedInvitation, R::SponsorRejectedInvitation),
            (E::SponsorAdmissionUnavailable, R::SponsorInternal),
            (E::SponsorAdmissionConflict, R::SponsorAdmissionConflict),
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
