use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use uc_core::membership::{
    AdmissionActivatedSecurityState, AdmissionActivationReceipt, AdmissionAppliedV1,
    AdmissionBaseSnapshot, AdmissionCandidateV1, AdmissionChangeFacts, AdmissionChannelPeerId,
    AdmissionCommitV1, AdmissionCompleteAckV1, AdmissionCompleteV1, AdmissionCompletionV1,
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent,
    AdmissionIdentitySignature, AdmissionInvitationClaim, AdmissionJoinRequestV1,
    AdmissionJoinerPrivateState, AdmissionJoinerStartContext, AdmissionKeyPackage,
    AdmissionMessageId, AdmissionMlsCommit, AdmissionMlsWelcome, AdmissionPeerBinding,
    AdmissionPreparedV1, AdmissionRecordPersistence, AdmissionRecoveryPublicKey,
    AdmissionRetryState, AdmissionRole, AdmissionSealedRecoveryMaterial,
    AdmissionSealedSecurityState, AdmissionSecurityCommitmentV1, AdmissionSettledV1,
    AdmissionSignedMembershipHistory, AdmissionSourceSnapshot, AdmissionSpaceTransitionResult,
    AdmissionStagedSecurityState, AdmissionStagedTarget, AdmissionStagedTargetInput,
    BaseMembershipHistoryPosition, InvitationId, JoinId, JoinerAdmission,
    JoinerAdmissionTransition, MemberInstanceId, MembershipAdmissionV2, MembershipCredential,
    MembershipEventV2, MembershipOperationV2, PendingAdmissionExchange, PreparedAdmissionProofV1,
    SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionMessageKind,
    SpaceAdmissionRoute, SponsorAdmission, UnreadableHistoryPolicy,
    ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
    MEMBERSHIP_EVENT_FORMAT_V2,
};
use uc_core::pairing::invitation::FullInvitation;
use uc_core::ports::SettingsPort;
use uc_core::security::IdentityFingerprint;

fn join_request_identity_facts(
    device_id: DeviceId,
    credential: &MembershipCredential,
    signature: Vec<u8>,
) -> AdmissionChangeFacts {
    AdmissionChangeFacts {
        member_instance: credential.member_instance_id(&device_id),
        device_id,
        device_name: "Joining device".to_owned(),
        identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
            .expect("valid fingerprint fixture"),
        transport_public_key: vec![0x53; 32],
        transport_address_blob: vec![0x54; 32],
        identity_signature: signature,
    }
}
use uc_core::DeviceId;

use super::{
    ActivateSponsorAdmissionError, ActivateSponsorAdmissionPort, AdmissionRecoveryCommitToken,
    AdmissionRecoveryService, AdmissionRecoveryTrigger, AuthenticatedAdmissionExchangePort,
    AuthenticatedAdmissionReply, AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    CompletedJoinerActivation, CurrentJoinAdmissionStatePort, ExecuteJoinerActivationError,
    ExecuteJoinerActivationPort, JoinerActivationCommitToken, JoinerActivationMutation,
    JoinerActivationOutcome, JoinerActivationStateError, JoinerActivationStatePort,
    JoinerAdmissionService, JoinerCancellationCommitToken, JoinerCancellationMaterial,
    JoinerCancellationMaterialError, JoinerCancellationMutation, JoinerCancellationStateError,
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedCurrentJoin, LoadedJoinerActivation,
    LoadedJoinerStartState, LoadedPendingAdmission, LoadedSponsorAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PrepareJoinerAppliedError,
    PrepareJoinerAppliedPort, PrepareJoinerCancellationPort, PrepareJoinerCandidateError,
    PrepareJoinerCandidatePort, PrepareJoinerInvitationError, PrepareJoinerInvitationPort,
    PrepareSponsorCandidateError, PrepareSponsorCandidatePort, PrepareSponsorCommitError,
    PrepareSponsorCommitPort, PrepareSponsorCompleteError, PrepareSponsorCompletePort,
    PrepareSponsorSettledError, PrepareSponsorSettledPort, PreparedJoinerActivation,
    PreparedJoinerAppliedMaterial, PreparedJoinerCandidateMaterial, PreparedJoinerInvitation,
    PreparedSponsorCandidate, PreparedSponsorCommit, PreparedSponsorComplete,
    PreparedSponsorSettled, ResolveJoinerInvitationError, ResolveJoinerInvitationPort,
    SpaceAdmissionCommitToken, SpaceAdmissionProtocol, SpaceAdmissionTransportError,
    SpaceAdmissionTransportPort, SponsorAdmissionCommitToken, SponsorAdmissionMutation,
    SponsorAdmissionService, SponsorAdmissionState, SponsorAdmissionStateError,
    SponsorAdmissionStatePort,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtocolEvent {
    DeviceNameSaved,
    JoinerSavedUnresolvedInvitation,
    JoinerInvitationResolutionStarted,
    JoinerInvitationResolutionRequested,
    JoinerSavedResolvedInvitation,
    JoinerRejectedConsumedInvitation,
    JoinerSavedJoinRequest,
    AdmissionRecoveryWoken,
    JoinerInitialChannelRequested,
    JoinerAuthenticatedChannelSaved,
    JoinerJoinRequestExchanged,
    JoinerSavedCandidate,
    JoinerSavedPrepared,
    JoinerContinuationChannelRequested,
    JoinerPreparedExchanged,
    JoinerAppliedExchanged,
    JoinerSavedCommitted,
    JoinerSavedApplied,
    JoinerSavedActivating,
    JoinerActivationExecuted,
    JoinerSavedActivePendingSettlement,
    JoinerCompleteAckExchanged,
    JoinerSavedActiveSettled,
    SponsorSavedAccepted,
    SponsorSavedCandidate,
    SponsorSavedCommitted,
    SponsorSavedApplied,
    SponsorSavedCompleted,
}

pub(super) struct SpaceAdmissionProtocolTestPair {
    joiner: SpaceAdmissionProtocol,
    sponsor: SpaceAdmissionProtocol,
    state: Arc<RecordingJoinerStartState>,
    sponsor_state: Arc<RecordingSponsorState>,
}

struct FixedJoinerStartMaterial;
struct FixedJoinerInvitationPreparation;
struct FixedJoinerInvitationResolver {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
}

struct UnusedSponsorPorts;

#[async_trait]
impl crate::space::membership::ResolveRePairingPort for UnusedSponsorPorts {
    async fn resolve_after_successful_pairing(
        &self,
    ) -> Result<(), crate::space::membership::RePairingStateError> {
        Ok(())
    }
}

struct RecordingSponsorState {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
    current: Mutex<Option<SponsorAdmission>>,
}

struct FixedSponsorCandidate;

struct FixedSponsorCommit;

struct FixedSponsorComplete;

struct FixedSponsorSettled;

struct FixedJoinerCandidate;

struct FixedJoinerApplied;

struct FixedJoinerCancellation;

struct FixedJoinerActivation {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
}

#[derive(Clone, Copy)]
enum TransportMode {
    DeferInitial,
    AuthenticateThenDefer,
    AuthenticateThenCandidate,
    AuthenticateThenCandidateAndCommit,
    AuthenticateThenCandidateCommitAndComplete,
}

struct RecordingMaintenanceWake {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
}

struct RecordingSpaceAdmissionTransport {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
    mode: TransportMode,
}

struct ExchangeThenDeferred {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
    continuation: Option<AdmissionContinuationCredential>,
    candidate_reply: bool,
    commit_reply: bool,
    complete_reply: bool,
    settled_reply: bool,
}

struct RecordingJoinerStartState {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
    current_join: Mutex<Option<JoinerAdmission>>,
    created_join: Mutex<Option<JoinerAdmission>>,
    superseded: AtomicBool,
    fail_next_activation_commit: AtomicBool,
}

struct RecordingSettings {
    value: Mutex<uc_core::settings::model::Settings>,
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
}

impl crate::space::membership::WakeSpaceMembershipMaintenancePort for RecordingMaintenanceWake {
    fn wake(&self) {
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::AdmissionRecoveryWoken);
    }
}

#[async_trait]
impl PrepareJoinerCancellationPort for FixedJoinerCancellation {
    async fn prepare(&self) -> Result<JoinerCancellationMaterial, JoinerCancellationMaterialError> {
        Ok(JoinerCancellationMaterial::new(
            AdmissionMessageId::from_bytes([0xd1; 32]).expect("valid cancellation message id"),
            AdmissionRetryState::new(0, 0).expect("valid cancellation retry state"),
        ))
    }
}

#[async_trait]
impl SponsorAdmissionStatePort for UnusedSponsorPorts {
    async fn load(
        &self,
        _message: &AuthenticatedSpaceAdmissionMessage,
    ) -> Result<LoadedSponsorAdmission, SponsorAdmissionStateError> {
        unreachable!()
    }

    async fn commit(
        &self,
        _token: SponsorAdmissionCommitToken,
        _mutation: SponsorAdmissionMutation,
    ) -> Result<CommittedSponsorAdmission, SponsorAdmissionStateError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSponsorCandidatePort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSponsorCommitPort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::SponsorCommitPreparation<'_>,
        _prepared: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorCommit, PrepareSponsorCommitError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSponsorCompletePort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::SponsorCompletePreparation<'_>,
        _applied: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorComplete, PrepareSponsorCompleteError> {
        unreachable!()
    }
}

#[async_trait]
impl ActivateSponsorAdmissionPort for UnusedSponsorPorts {
    async fn activate(
        &self,
        _activated_security: &AdmissionActivatedSecurityState,
    ) -> Result<(), ActivateSponsorAdmissionError> {
        unreachable!()
    }
}

#[async_trait]
impl ActivateSponsorAdmissionPort for FixedSponsorComplete {
    async fn activate(
        &self,
        _activated_security: &AdmissionActivatedSecurityState,
    ) -> Result<(), ActivateSponsorAdmissionError> {
        Ok(())
    }
}

#[async_trait]
impl PrepareSponsorSettledPort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::SponsorSettlementPreparation<'_>,
        _complete_ack: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorSettled, PrepareSponsorSettledError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinerActivationPort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::JoinerCompletePreparation<'_>,
        _complete: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerActivation, PrepareJoinerActivationError> {
        unreachable!()
    }
}

#[async_trait]
impl JoinerActivationStatePort for UnusedSponsorPorts {
    async fn load(&self) -> Result<Option<LoadedJoinerActivation>, JoinerActivationStateError> {
        unreachable!()
    }

    async fn commit(
        &self,
        _token: JoinerActivationCommitToken,
        _mutation: JoinerActivationMutation,
    ) -> Result<(), JoinerActivationStateError> {
        unreachable!()
    }
}

#[async_trait]
impl ExecuteJoinerActivationPort for UnusedSponsorPorts {
    async fn execute(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError> {
        unreachable!()
    }
}

#[async_trait]
impl SettingsPort for RecordingSettings {
    async fn load(&self) -> anyhow::Result<uc_core::settings::model::Settings> {
        Ok(self.value.lock().expect("settings are available").clone())
    }

    async fn save(&self, settings: &uc_core::settings::model::Settings) -> anyhow::Result<()> {
        *self.value.lock().expect("settings are available") = settings.clone();
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::DeviceNameSaved);
        Ok(())
    }
}

#[async_trait]
impl JoinerStartStatePort for RecordingJoinerStartState {
    async fn load(&self) -> Result<LoadedJoinerStartState, JoinerStartStateError> {
        Ok(LoadedJoinerStartState::new(
            7,
            AdmissionSourceSnapshot::from_bytes(vec![0x24; 32]).expect("valid source snapshot"),
            self.current_join
                .lock()
                .expect("current join is available")
                .take(),
            true,
            SpaceAdmissionCommitToken::from_bytes([0x25; 32]).expect("valid commit token"),
        ))
    }

    async fn commit(
        &self,
        token: SpaceAdmissionCommitToken,
        mutation: JoinerStartMutation,
    ) -> Result<(), JoinerStartStateError> {
        assert_eq!(token.as_bytes(), &[0x25; 32]);
        let (created, superseded) = mutation.into_parts();
        assert_eq!(created.replacement().record_version(), 0);
        self.superseded
            .store(superseded.is_some(), Ordering::SeqCst);
        let event = if matches!(
            created.replacement().invitation_resolution(),
            Some(uc_core::membership::JoinerInvitationResolution::Ready { .. })
        ) {
            ProtocolEvent::JoinerSavedUnresolvedInvitation
        } else {
            ProtocolEvent::JoinerSavedJoinRequest
        };
        *self.created_join.lock().expect("created join is available") =
            Some(created.into_replacement());
        self.events
            .lock()
            .expect("event recorder is available")
            .push(event);
        Ok(())
    }
}

#[async_trait]
impl CurrentJoinAdmissionStatePort for RecordingJoinerStartState {
    async fn load(
        &self,
        join_id: JoinId,
    ) -> Result<Option<LoadedCurrentJoin>, JoinerCancellationStateError> {
        let stored = self.created_join.lock().expect("created join is available");
        let Some(admission) = stored.as_ref() else {
            return Ok(None);
        };
        if admission.join_id() != join_id {
            return Ok(None);
        }
        let persisted = admission
            .encode_persisted()
            .expect("current join can be persisted");
        let admission =
            JoinerAdmission::decode_persisted(&persisted).expect("current join can be reopened");
        Ok(Some(LoadedCurrentJoin::new(
            admission,
            JoinerCancellationCommitToken::from_bytes([0xd2; 32])
                .expect("valid cancellation token"),
        )))
    }

    async fn commit(
        &self,
        token: JoinerCancellationCommitToken,
        mutation: JoinerCancellationMutation,
    ) -> Result<(), JoinerCancellationStateError> {
        assert_eq!(token.as_bytes(), &[0xd2; 32]);
        let replacement = mutation.into_transition().into_replacement();
        let persisted = replacement
            .encode_persisted()
            .expect("cancelled join can be persisted");
        *self.created_join.lock().expect("created join is available") = Some(
            JoinerAdmission::decode_persisted(&persisted).expect("cancelled join can be reopened"),
        );
        Ok(())
    }
}

#[async_trait]
impl PendingAdmissionRecoveryStatePort for RecordingJoinerStartState {
    async fn load(
        &self,
        _trigger: AdmissionRecoveryTrigger,
    ) -> Result<Vec<LoadedPendingAdmission>, PendingAdmissionRecoveryStateError> {
        let stored = self.created_join.lock().expect("created join is available");
        let Some(aggregate) = stored.as_ref() else {
            return Ok(Vec::new());
        };
        let persisted = aggregate
            .encode_persisted()
            .expect("pending aggregate can be persisted");
        let aggregate = JoinerAdmission::decode_persisted(&persisted)
            .expect("pending aggregate can be reopened");
        Ok(vec![LoadedPendingAdmission::new(
            aggregate,
            AdmissionRecoveryCommitToken::from_bytes([0x26; 32])
                .expect("valid recovery commit token"),
        )])
    }

    async fn commit(
        &self,
        _token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let effects = transition.effects();
        let aggregate = transition.into_replacement();
        let resolution_event = match aggregate.invitation_resolution() {
            Some(uc_core::membership::JoinerInvitationResolution::Started { .. }) => {
                Some((ProtocolEvent::JoinerInvitationResolutionStarted, 0x27))
            }
            Some(uc_core::membership::JoinerInvitationResolution::Resolved { .. }) => {
                Some((ProtocolEvent::JoinerSavedResolvedInvitation, 0x28))
            }
            _ => None,
        };
        let terminal_resolution_event = (aggregate.is_terminal()
            && aggregate.record_version() == 2)
            .then_some((ProtocolEvent::JoinerRejectedConsumedInvitation, 0x28));
        let (event, next_token_byte) =
            if let Some(event) = resolution_event.or(terminal_resolution_event) {
                assert!(effects.is_empty());
                event
            } else {
                match aggregate.record_version() {
                    1 => {
                        assert!(effects.is_empty());
                        (ProtocolEvent::JoinerAuthenticatedChannelSaved, 0x27)
                    }
                    2 => {
                        assert!(effects.is_empty());
                        (ProtocolEvent::JoinerSavedCandidate, 0x28)
                    }
                    3 => {
                        assert!(effects.is_empty());
                        (ProtocolEvent::JoinerSavedPrepared, 0x29)
                    }
                    4 => {
                        assert!(effects.is_empty());
                        (ProtocolEvent::JoinerSavedCommitted, 0x2a)
                    }
                    5 => {
                        assert_eq!(
                            effects,
                            &[uc_core::membership::AdmissionEffect::ApplyMembership]
                        );
                        (ProtocolEvent::JoinerSavedApplied, 0x2b)
                    }
                    6 => {
                        assert_eq!(
                            effects,
                            &[uc_core::membership::AdmissionEffect::ActivateSpace]
                        );
                        (ProtocolEvent::JoinerSavedActivating, 0x2c)
                    }
                    8 if aggregate.is_active_settled() => {
                        assert!(effects.is_empty());
                        (ProtocolEvent::JoinerSavedActiveSettled, 0x2e)
                    }
                    _ => return Err(PendingAdmissionRecoveryStateError::RecoveryRequired),
                }
            };
        let persisted = aggregate
            .encode_persisted()
            .expect("test aggregate can be persisted");
        *self.created_join.lock().expect("created join is available") = Some(
            JoinerAdmission::decode_persisted(&persisted).expect("test aggregate can be reopened"),
        );
        self.events
            .lock()
            .expect("event recorder is available")
            .push(event);
        Ok(LoadedPendingAdmission::new(
            aggregate,
            AdmissionRecoveryCommitToken::from_bytes([next_token_byte; 32])
                .expect("valid next recovery commit token"),
        ))
    }
}

#[async_trait]
impl JoinerActivationStatePort for RecordingJoinerStartState {
    async fn load(&self) -> Result<Option<LoadedJoinerActivation>, JoinerActivationStateError> {
        let stored = self.created_join.lock().expect("created join is available");
        let Some(aggregate) = stored.as_ref() else {
            return Ok(None);
        };
        if aggregate.joiner_activation_preparation().is_none() {
            return Ok(None);
        }
        let persisted = aggregate
            .encode_persisted()
            .expect("activating aggregate can be persisted");
        let aggregate = JoinerAdmission::decode_persisted(&persisted)
            .expect("activating aggregate can be reopened");
        Ok(Some(LoadedJoinerActivation::new(
            aggregate,
            JoinerActivationCommitToken::from_bytes([0xbd; 32])
                .expect("valid activation commit token"),
        )))
    }

    async fn commit(
        &self,
        token: JoinerActivationCommitToken,
        mutation: JoinerActivationMutation,
    ) -> Result<(), JoinerActivationStateError> {
        assert_eq!(token.as_bytes(), &[0xbd; 32]);
        let transition = mutation.into_transition();
        assert_eq!(
            transition.effects(),
            &[uc_core::membership::AdmissionEffect::PublishActive]
        );
        let aggregate = transition.into_replacement();
        assert_eq!(aggregate.record_version(), 7);
        if self
            .fail_next_activation_commit
            .swap(false, Ordering::SeqCst)
        {
            return Err(JoinerActivationStateError::state_changed(anyhow::anyhow!(
                "simulated activation commit conflict"
            )));
        }
        let persisted = aggregate
            .encode_persisted()
            .expect("active aggregate can be persisted");
        *self.created_join.lock().expect("created join is available") = Some(
            JoinerAdmission::decode_persisted(&persisted)
                .expect("active aggregate can be reopened"),
        );
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::JoinerSavedActivePendingSettlement);
        Ok(())
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for RecordingSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        assert_eq!(admission_id.as_bytes(), &[0x11; 32]);
        assert_eq!(route.as_bytes(), &[0x19; 32]);
        assert_eq!(encrypted_password_equivalent.as_bytes(), &[0x1a; 64]);
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::JoinerInitialChannelRequested);
        if matches!(self.mode, TransportMode::DeferInitial) {
            return Err(SpaceAdmissionTransportError::Deferred);
        }
        Ok(Box::new(ExchangeThenDeferred {
            events: Arc::clone(&self.events),
            continuation: Some(
                AdmissionContinuationCredential::from_bytes(vec![0x32; 64])
                    .expect("valid continuation credential"),
            ),
            candidate_reply: matches!(
                self.mode,
                TransportMode::AuthenticateThenCandidate
                    | TransportMode::AuthenticateThenCandidateAndCommit
                    | TransportMode::AuthenticateThenCandidateCommitAndComplete
            ),
            commit_reply: false,
            complete_reply: false,
            settled_reply: false,
        }))
    }

    async fn resume(
        &self,
        _admission_id: SpaceAdmissionId,
        _route: &SpaceAdmissionRoute,
        _peer_binding: AdmissionPeerBinding,
        _continuation_credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        if !matches!(
            self.mode,
            TransportMode::AuthenticateThenCandidateAndCommit
                | TransportMode::AuthenticateThenCandidateCommitAndComplete
        ) {
            return Err(SpaceAdmissionTransportError::Deferred);
        }
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::JoinerContinuationChannelRequested);
        Ok(Box::new(ExchangeThenDeferred {
            events: Arc::clone(&self.events),
            continuation: None,
            candidate_reply: false,
            commit_reply: true,
            complete_reply: matches!(
                self.mode,
                TransportMode::AuthenticateThenCandidateCommitAndComplete
            ),
            settled_reply: matches!(
                self.mode,
                TransportMode::AuthenticateThenCandidateCommitAndComplete
            ),
        }))
    }
}

#[async_trait]
impl AuthenticatedAdmissionExchangePort for ExchangeThenDeferred {
    fn peer_binding(&self) -> AdmissionPeerBinding {
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0x33; 32]).expect("valid local peer"),
            AdmissionChannelPeerId::from_bytes([0x34; 32]).expect("valid remote peer"),
        )
        .expect("distinct peers")
    }

    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential> {
        self.continuation.take()
    }

    async fn exchange(
        self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
        assert_eq!(request.header().admission_id().as_bytes(), &[0x11; 32]);
        if request.kind() == SpaceAdmissionMessageKind::JoinRequest {
            assert_eq!(request.header().message_id().as_bytes(), &[0x18; 32]);
            self.events
                .lock()
                .expect("event recorder is available")
                .push(ProtocolEvent::JoinerJoinRequestExchanged);
            if !self.candidate_reply {
                return Err(SpaceAdmissionTransportError::Deferred);
            }
            let candidate = SpaceAdmissionEnvelopeV1::new(
                request.header().admission_id(),
                AdmissionRole::Sponsor,
                0,
                AdmissionMessageId::from_bytes([0x7c; 32]).expect("valid candidate message id"),
                Some(request.header().message_id()),
                SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
            )
            .expect("valid candidate reply");
            return Ok(AuthenticatedAdmissionReply::new(candidate, [0x7d; 32])
                .expect("valid authenticated reply"));
        }
        if self.commit_reply && request.kind() == SpaceAdmissionMessageKind::Prepared {
            self.events
                .lock()
                .expect("event recorder is available")
                .push(ProtocolEvent::JoinerPreparedExchanged);
            let commit = SpaceAdmissionEnvelopeV1::new(
                request.header().admission_id(),
                AdmissionRole::Sponsor,
                1,
                AdmissionMessageId::from_bytes([0x9b; 32]).expect("valid Commit message id"),
                Some(request.header().message_id()),
                SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                    candidate_body_fixture(),
                    AdmissionSignedMembershipHistory::from_bytes(vec![0x89; 128])
                        .expect("valid committed target history"),
                    AdmissionSealedRecoveryMaterial::from_bytes(vec![0x9c; 128])
                        .expect("valid sealed recovery material"),
                )),
            )
            .expect("valid Commit reply");
            return Ok(AuthenticatedAdmissionReply::new(commit, [0x9d; 32])
                .expect("valid authenticated Commit"));
        }
        if self.complete_reply && request.kind() == SpaceAdmissionMessageKind::Applied {
            self.events
                .lock()
                .expect("event recorder is available")
                .push(ProtocolEvent::JoinerAppliedExchanged);
            let SpaceAdmissionBodyV1::Applied(applied_body) = request.body() else {
                return Err(SpaceAdmissionTransportError::ProtocolRejected);
            };
            let receipt = applied_body.activation_receipt();
            let completion = AdmissionCompletionV1::new(
                *request.header().admission_id().as_bytes(),
                receipt.event_id,
                [0xb1; 32],
                receipt.installed_security_commitment_id,
                MemberInstanceId::from_bytes([0xb2; 32]),
                MembershipCredential::new(1, vec![0xb3; 32]).credential_id,
                BaseMembershipHistoryPosition {
                    event_id: Some(receipt.event_id),
                    depth: 1,
                    history_digest: [0xb4; 32],
                },
                vec![0xb5; 64],
            );
            let complete = SpaceAdmissionEnvelopeV1::new(
                request.header().admission_id(),
                AdmissionRole::Sponsor,
                2,
                AdmissionMessageId::from_bytes([0xb6; 32]).expect("valid Complete message id"),
                Some(request.header().message_id()),
                SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
            )
            .expect("valid Complete reply");
            return Ok(AuthenticatedAdmissionReply::new(complete, [0xb7; 32])
                .expect("valid authenticated Complete"));
        }
        if self.settled_reply && request.kind() == SpaceAdmissionMessageKind::CompleteAck {
            self.events
                .lock()
                .expect("event recorder is available")
                .push(ProtocolEvent::JoinerCompleteAckExchanged);
            let settled = SpaceAdmissionEnvelopeV1::new(
                request.header().admission_id(),
                AdmissionRole::Sponsor,
                3,
                AdmissionMessageId::from_bytes([0xc5; 32]).expect("valid Settled message id"),
                Some(request.header().message_id()),
                SpaceAdmissionBodyV1::Settled(
                    AdmissionSettledV1::new([0xc6; 32]).expect("valid completion digest"),
                ),
            )
            .expect("valid Settled reply");
            return Ok(AuthenticatedAdmissionReply::new(settled, [0xc7; 32])
                .expect("valid authenticated Settled"));
        }
        Err(SpaceAdmissionTransportError::Deferred)
    }
}

#[async_trait]
impl PrepareJoinerCandidatePort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _preparation: uc_core::membership::JoinerCandidatePreparation<'_>,
        _candidate: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinerAppliedPort for UnusedSponsorPorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: uc_core::membership::JoinerAppliedPreparation<'_>,
    ) -> Result<PreparedJoinerAppliedMaterial, PrepareJoinerAppliedError> {
        unreachable!()
    }
}

#[async_trait]
impl SponsorAdmissionStatePort for RecordingSponsorState {
    async fn load(
        &self,
        _message: &AuthenticatedSpaceAdmissionMessage,
    ) -> Result<LoadedSponsorAdmission, SponsorAdmissionStateError> {
        let state = self
            .current
            .lock()
            .expect("sponsor state is available")
            .take()
            .map(SponsorAdmissionState::Existing)
            .unwrap_or_else(|| SponsorAdmissionState::Fresh {
                invitation_claim: AdmissionInvitationClaim::from_bytes(vec![0x41; 32])
                    .expect("valid invitation claim"),
                base_snapshot: AdmissionBaseSnapshot::from_bytes(vec![0x42; 64])
                    .expect("valid base snapshot"),
            });
        Ok(LoadedSponsorAdmission::new(
            state,
            SponsorAdmissionCommitToken::from_bytes([0x43; 32])
                .expect("valid sponsor commit token"),
        ))
    }

    async fn commit(
        &self,
        _token: SponsorAdmissionCommitToken,
        mutation: SponsorAdmissionMutation,
    ) -> Result<CommittedSponsorAdmission, SponsorAdmissionStateError> {
        let transition = mutation.into_transition();
        let effects = transition.effects();
        let aggregate = transition.into_replacement();
        let event = match aggregate.current_exact_reply().map(|reply| reply.kind()) {
            None => {
                assert_eq!(
                    effects,
                    &[uc_core::membership::AdmissionEffect::ConsumeInvitation]
                );
                ProtocolEvent::SponsorSavedAccepted
            }
            Some(SpaceAdmissionMessageKind::Candidate) => {
                assert!(effects.is_empty());
                ProtocolEvent::SponsorSavedCandidate
            }
            Some(SpaceAdmissionMessageKind::Commit) => {
                assert_eq!(
                    effects,
                    &[uc_core::membership::AdmissionEffect::CommitMembership]
                );
                ProtocolEvent::SponsorSavedCommitted
            }
            Some(SpaceAdmissionMessageKind::Complete) => {
                assert_eq!(
                    effects,
                    &[
                        uc_core::membership::AdmissionEffect::ActivateSecurity,
                        uc_core::membership::AdmissionEffect::PublishMembership,
                    ]
                );
                ProtocolEvent::SponsorSavedApplied
            }
            Some(SpaceAdmissionMessageKind::Settled) => {
                assert!(effects.is_empty());
                ProtocolEvent::SponsorSavedCompleted
            }
            _ => {
                return Err(SponsorAdmissionStateError::recovery_required(
                    anyhow::anyhow!("unexpected Sponsor aggregate state in test recorder"),
                ));
            }
        };
        self.events
            .lock()
            .expect("event recorder is available")
            .push(event);
        Ok(CommittedSponsorAdmission::new(
            aggregate,
            SponsorAdmissionCommitToken::from_bytes([0x44; 32])
                .expect("valid next sponsor commit token"),
        ))
    }
}

#[async_trait]
impl PrepareSponsorCandidatePort for FixedSponsorCandidate {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError> {
        let candidate = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            0,
            AdmissionMessageId::from_bytes([0x45; 32]).expect("valid candidate message id"),
            Some(preparation.join_request().header().message_id()),
            SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
        )
        .expect("valid candidate reply");
        Ok(PreparedSponsorCandidate::new(
            candidate,
            AdmissionStagedSecurityState::from_bytes(vec![0x46; 128])
                .expect("valid staged security"),
        ))
    }
}

#[async_trait]
impl PrepareSponsorCommitPort for FixedSponsorCommit {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::SponsorCommitPreparation<'_>,
        prepared: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorCommit, PrepareSponsorCommitError> {
        assert_eq!(
            preparation.candidate_reply().kind(),
            SpaceAdmissionMessageKind::Candidate
        );
        assert!(!preparation.base_snapshot().as_bytes().is_empty());
        assert!(!preparation.staged_security().as_bytes().is_empty());
        let committed_history_bytes = vec![0x97; 128];
        let commit = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            1,
            AdmissionMessageId::from_bytes([0x98; 32]).expect("valid Commit message id"),
            Some(prepared.header().message_id()),
            SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                candidate_body_fixture(),
                AdmissionSignedMembershipHistory::from_bytes(committed_history_bytes.clone())
                    .expect("valid target history"),
                AdmissionSealedRecoveryMaterial::from_bytes(vec![0x99; 128])
                    .expect("valid sealed recovery material"),
            )),
        )
        .expect("valid Commit reply");
        Ok(PreparedSponsorCommit::new(
            AdmissionSignedMembershipHistory::from_bytes(committed_history_bytes)
                .expect("valid committed history"),
            AdmissionSealedSecurityState::from_bytes(vec![0x9a; 128])
                .expect("valid sealed security"),
            commit,
        ))
    }
}

#[async_trait]
impl PrepareSponsorCompletePort for FixedSponsorComplete {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::SponsorCompletePreparation<'_>,
        applied: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorComplete, PrepareSponsorCompleteError> {
        assert_eq!(
            preparation.commit_reply().kind(),
            SpaceAdmissionMessageKind::Commit
        );
        assert!(!preparation.committed_history().as_bytes().is_empty());
        assert!(!preparation.sealed_security().as_bytes().is_empty());
        let SpaceAdmissionBodyV1::Applied(applied_body) = applied.body() else {
            return Err(PrepareSponsorCompleteError::invalid(anyhow::anyhow!(
                "fixture requires an Applied message"
            )));
        };
        let receipt = applied_body.activation_receipt();
        let completion = AdmissionCompletionV1::new(
            *admission_id.as_bytes(),
            receipt.event_id,
            [0xaa; 32],
            receipt.installed_security_commitment_id,
            MemberInstanceId::from_bytes([0xab; 32]),
            MembershipCredential::new(1, vec![0xac; 32]).credential_id,
            BaseMembershipHistoryPosition {
                event_id: Some(receipt.event_id),
                depth: 1,
                history_digest: [0xad; 32],
            },
            vec![0xae; 64],
        );
        let complete = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            2,
            AdmissionMessageId::from_bytes([0xaf; 32]).expect("valid Complete message id"),
            Some(applied.header().message_id()),
            SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
        )
        .expect("valid Complete reply");
        Ok(PreparedSponsorComplete::new(
            AdmissionActivatedSecurityState::from_bytes(vec![0xb0; 128])
                .expect("valid activated security"),
            complete,
        ))
    }
}

#[async_trait]
impl PrepareSponsorSettledPort for FixedSponsorSettled {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::SponsorSettlementPreparation<'_>,
        complete_ack: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorSettled, PrepareSponsorSettledError> {
        assert_eq!(
            preparation.complete_reply().kind(),
            SpaceAdmissionMessageKind::Complete
        );
        let settled = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            3,
            AdmissionMessageId::from_bytes([0xc5; 32]).expect("valid Settled message id"),
            Some(complete_ack.header().message_id()),
            SpaceAdmissionBodyV1::Settled(
                AdmissionSettledV1::new([0xc6; 32]).expect("valid CompleteAck digest"),
            ),
        )
        .expect("valid Settled reply");
        Ok(PreparedSponsorSettled::new(settled))
    }
}

#[async_trait]
impl PrepareJoinerCandidatePort for FixedJoinerCandidate {
    async fn prepare(
        &self,
        preparation: uc_core::membership::JoinerCandidatePreparation<'_>,
        candidate: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError> {
        assert_eq!(preparation.private_state().as_bytes(), &[0x1b; 64]);
        let SpaceAdmissionBodyV1::Candidate(candidate_body) = candidate.body() else {
            return Err(PrepareJoinerCandidateError::Invalid);
        };
        let admission_id = candidate.header().admission_id();
        let prepared = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            1,
            AdmissionMessageId::from_bytes([0x81; 32]).expect("valid Prepared message id"),
            Some(candidate.header().message_id()),
            SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(
                PreparedAdmissionProofV1::new(
                    *admission_id.as_bytes(),
                    "lineage".to_owned(),
                    BaseMembershipHistoryPosition {
                        event_id: None,
                        depth: 0,
                        history_digest: [0x82; 32],
                    },
                    candidate_body.candidate_event().event_id(),
                    candidate_body.candidate_event().resulting_members_digest,
                    candidate_body.security_commitment().security_commitment_id,
                    MemberInstanceId::from_bytes([0x84; 32]),
                    MembershipCredential::new(1, vec![0x85; 32]).credential_id,
                    vec![0x86; 64],
                ),
            )),
        )
        .expect("valid Prepared request");
        let prepared_exchange = PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(vec![0x87; 32]).expect("valid continuation route"),
            prepared,
            SpaceAdmissionMessageKind::Commit,
            AdmissionRetryState::new(0, 0).expect("valid retry state"),
        )
        .expect("Prepared expects Commit");
        Ok(PreparedJoinerCandidateMaterial::new(
            AdmissionStagedTargetInput::from_bytes(vec![0x88; 128])
                .expect("valid staged target input"),
            AdmissionSignedMembershipHistory::from_bytes(vec![0x89; 128])
                .expect("valid verified history"),
            AdmissionStagedTarget::from_bytes(vec![0x8a; 128]).expect("valid staged target"),
            prepared_exchange,
        ))
    }
}

#[async_trait]
impl PrepareJoinerAppliedPort for FixedJoinerApplied {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::JoinerAppliedPreparation<'_>,
    ) -> Result<PreparedJoinerAppliedMaterial, PrepareJoinerAppliedError> {
        assert!(!preparation.staged_target().as_bytes().is_empty());
        let SpaceAdmissionBodyV1::Commit(commit) = preparation.exact_commit().body() else {
            return Err(PrepareJoinerAppliedError::invalid(anyhow::anyhow!(
                "fixture requires an exact Commit"
            )));
        };
        let candidate = commit.exact_candidate();
        let receipt = AdmissionActivationReceipt::new(
            1,
            *admission_id.as_bytes(),
            candidate.candidate_event().event_id(),
            [0x9e; 32],
            candidate.security_commitment().security_commitment_id,
            MemberInstanceId::from_bytes([0x9f; 32]),
            vec![0xa0; 64],
        );
        let applied = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            2,
            AdmissionMessageId::from_bytes([0xa1; 32]).expect("valid Applied message id"),
            Some(preparation.exact_commit().header().message_id()),
            SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt)),
        )
        .expect("valid Applied request");
        let pending_exchange = PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(vec![0xa2; 32])
                .expect("valid Applied continuation route"),
            applied,
            SpaceAdmissionMessageKind::Complete,
            AdmissionRetryState::new(0, 0).expect("valid retry state"),
        )
        .expect("Applied expects Complete");
        Ok(PreparedJoinerAppliedMaterial::new(pending_exchange))
    }
}

#[async_trait]
impl PrepareJoinerActivationPort for FixedJoinerActivation {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::JoinerCompletePreparation<'_>,
        complete: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerActivation, PrepareJoinerActivationError> {
        assert!(!preparation.source_snapshot().as_bytes().is_empty());
        assert!(!preparation.staged_target().as_bytes().is_empty());
        assert_eq!(
            preparation.exact_commit().kind(),
            SpaceAdmissionMessageKind::Commit
        );
        assert_eq!(
            preparation.applied_request().kind(),
            SpaceAdmissionMessageKind::Applied
        );
        assert_eq!(complete.kind(), SpaceAdmissionMessageKind::Complete);
        Ok(PreparedJoinerActivation::new(
            uc_core::membership::AdmissionSpaceTransition::from_bytes(vec![0xb8; 128])
                .expect("valid activation plan"),
        ))
    }
}

#[async_trait]
impl ExecuteJoinerActivationPort for FixedJoinerActivation {
    async fn execute(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError> {
        assert!(!preparation.space_transition().as_bytes().is_empty());
        assert!(!preparation.staged_target().as_bytes().is_empty());
        assert_eq!(
            preparation.exact_commit().kind(),
            SpaceAdmissionMessageKind::Commit
        );
        let completion = preparation.completion();
        assert_eq!(completion.kind(), SpaceAdmissionMessageKind::Complete);
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::JoinerActivationExecuted);
        let complete_ack = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            3,
            AdmissionMessageId::from_bytes([0xbe; 32]).expect("valid CompleteAck message id"),
            Some(completion.header().message_id()),
            SpaceAdmissionBodyV1::CompleteAck(
                AdmissionCompleteAckV1::new([0xbf; 32]).expect("valid completion digest"),
            ),
        )
        .expect("valid CompleteAck request");
        let pending_exchange = PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(vec![0xc0; 32])
                .expect("valid CompleteAck continuation route"),
            complete_ack,
            SpaceAdmissionMessageKind::Settled,
            AdmissionRetryState::new(0, 0).expect("valid retry state"),
        )
        .expect("CompleteAck expects Settled");
        Ok(CompletedJoinerActivation::new(
            AdmissionSpaceTransitionResult::from_bytes(vec![0xc1; 128])
                .expect("valid activation result"),
            pending_exchange,
            JoinerActivationOutcome {
                join_id: *preparation.join_id().as_bytes(),
                sponsor_device_id: DeviceId::new("sponsor-device"),
                sponsor_identity_fingerprint: IdentityFingerprint::from_display_string(
                    "1111-2222-3333-4444",
                )
                .expect("valid sponsor fingerprint"),
                space_id: "target-space".to_owned(),
                self_device_id: DeviceId::new("joining-device"),
                self_identity_fingerprint: IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .expect("valid joiner fingerprint"),
                migrated_records: None,
                preserved_unreadable_records: None,
            },
        ))
    }
}

impl SpaceAdmissionProtocolTestPair {
    pub(super) async fn fresh() -> Self {
        Self::with_mode(None, TransportMode::DeferInitial).await
    }

    pub(super) async fn short_invitation() -> Self {
        Self::fresh().await
    }

    pub(super) async fn authenticating() -> Self {
        Self::with_mode(None, TransportMode::AuthenticateThenDefer).await
    }

    pub(super) async fn receiving_candidate() -> Self {
        Self::with_mode(None, TransportMode::AuthenticateThenCandidate).await
    }

    pub(super) async fn receiving_commit() -> Self {
        Self::with_mode(None, TransportMode::AuthenticateThenCandidateAndCommit).await
    }

    pub(super) async fn receiving_complete() -> Self {
        Self::with_mode(
            None,
            TransportMode::AuthenticateThenCandidateCommitAndComplete,
        )
        .await
    }

    pub(super) async fn with_current_join(current_join: Option<JoinerAdmission>) -> Self {
        Self::with_mode(current_join, TransportMode::DeferInitial).await
    }

    async fn with_mode(current_join: Option<JoinerAdmission>, mode: TransportMode) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(RecordingJoinerStartState {
            events: Arc::clone(&events),
            current_join: Mutex::new(current_join),
            created_join: Mutex::new(None),
            superseded: AtomicBool::new(false),
            fail_next_activation_commit: AtomicBool::new(false),
        });
        let sponsor_state = Arc::new(RecordingSponsorState {
            events: Arc::clone(&events),
            current: Mutex::new(None),
        });
        let joiner_activation = Arc::new(FixedJoinerActivation {
            events: Arc::clone(&events),
        });
        Self {
            joiner: SpaceAdmissionProtocol::new(
                JoinerAdmissionService::new(
                    Arc::new(RecordingSettings {
                        value: Mutex::new(Default::default()),
                        events: Arc::clone(&events),
                    }),
                    Arc::new(FixedJoinerInvitationPreparation),
                    Arc::new(FixedJoinerInvitationResolver {
                        events: Arc::clone(&events),
                    }),
                    Arc::new(FixedJoinerStartMaterial),
                    state.clone(),
                    state.clone(),
                    Arc::new(FixedJoinerCancellation),
                    Arc::new(FixedJoinerCandidate),
                    Arc::new(FixedJoinerApplied),
                    joiner_activation.clone(),
                    state.clone(),
                    joiner_activation.clone(),
                    Arc::new(RecordingMaintenanceWake {
                        events: Arc::clone(&events),
                    }),
                    Arc::new(UnusedSponsorPorts),
                ),
                SponsorAdmissionService::new(
                    Arc::new(UnusedSponsorPorts),
                    Arc::new(UnusedSponsorPorts),
                    Arc::new(UnusedSponsorPorts),
                    Arc::new(UnusedSponsorPorts),
                    Arc::new(UnusedSponsorPorts),
                    Arc::new(UnusedSponsorPorts),
                    Arc::new(UnusedSponsorPorts),
                ),
                AdmissionRecoveryService::new(
                    state.clone(),
                    Arc::new(RecordingSpaceAdmissionTransport {
                        events: Arc::clone(&events),
                        mode,
                    }),
                ),
            ),
            sponsor: SpaceAdmissionProtocol::new(
                JoinerAdmissionService::new(
                    Arc::new(RecordingSettings {
                        value: Mutex::new(Default::default()),
                        events: Arc::clone(&events),
                    }),
                    Arc::new(FixedJoinerInvitationPreparation),
                    Arc::new(FixedJoinerInvitationResolver {
                        events: Arc::clone(&events),
                    }),
                    Arc::new(FixedJoinerStartMaterial),
                    state.clone(),
                    state.clone(),
                    Arc::new(FixedJoinerCancellation),
                    Arc::new(FixedJoinerCandidate),
                    Arc::new(FixedJoinerApplied),
                    joiner_activation.clone(),
                    state.clone(),
                    joiner_activation,
                    Arc::new(RecordingMaintenanceWake {
                        events: Arc::clone(&events),
                    }),
                    Arc::new(UnusedSponsorPorts),
                ),
                SponsorAdmissionService::new(
                    sponsor_state.clone(),
                    Arc::new(FixedSponsorCandidate),
                    Arc::new(FixedSponsorCommit),
                    Arc::new(FixedSponsorComplete),
                    Arc::new(FixedSponsorComplete),
                    Arc::new(FixedSponsorSettled),
                    Arc::new(UnusedSponsorPorts),
                ),
                AdmissionRecoveryService::new(
                    state.clone(),
                    Arc::new(RecordingSpaceAdmissionTransport {
                        events: Arc::clone(&events),
                        mode: TransportMode::DeferInitial,
                    }),
                ),
            ),
            state,
            sponsor_state,
        }
    }

    pub(super) fn joiner(&self) -> &SpaceAdmissionProtocol {
        &self.joiner
    }

    pub(super) fn sponsor(&self) -> &SpaceAdmissionProtocol {
        &self.sponsor
    }

    pub(super) fn seed_sponsor(&self, admission: SponsorAdmission) {
        *self
            .sponsor_state
            .current
            .lock()
            .expect("sponsor state is available") = Some(admission);
    }

    pub(super) fn events(&self) -> Vec<ProtocolEvent> {
        self.state.events.lock().unwrap().clone()
    }

    pub(super) fn take_created_join(&self) -> JoinerAdmission {
        self.state
            .created_join
            .lock()
            .expect("created join is available")
            .take()
            .expect("one join was committed")
    }

    pub(super) fn fail_next_activation_commit(&self) {
        self.state
            .fail_next_activation_commit
            .store(true, Ordering::SeqCst);
    }

    pub(super) fn superseded_previous_join(&self) -> bool {
        self.state.superseded.load(Ordering::SeqCst)
    }

    pub(super) fn simulate_invitation_resolution_started(&self) {
        let aggregate = self
            .state
            .created_join
            .lock()
            .expect("created join is available")
            .take()
            .expect("created join exists");
        let started = aggregate
            .mark_invitation_resolution_started()
            .expect("ready invitation can start");
        let (transition, _short_code) = started.into_parts();
        *self
            .state
            .created_join
            .lock()
            .expect("created join is available") = Some(transition.into_replacement());
    }

    pub(super) fn clear_events(&self) {
        self.state
            .events
            .lock()
            .expect("events are available")
            .clear();
    }
}

#[async_trait]
impl PrepareJoinerInvitationPort for FixedJoinerInvitationPreparation {
    async fn prepare(
        &self,
        input: &crate::space::admission::JoinSpaceInput,
    ) -> Result<PreparedJoinerInvitation, PrepareJoinerInvitationError> {
        if !input.invitation_code.as_str().starts_with("short-") {
            return Ok(PreparedJoinerInvitation::Full);
        }
        Ok(PreparedJoinerInvitation::short(
            SpaceAdmissionId::from_bytes([0x31; 32]).expect("valid admission id"),
            JoinId::from_bytes([0x32; 16]).expect("valid join id"),
            uc_core::membership::AdmissionJoinerStartContext::from_bytes(vec![0x33; 64])
                .expect("valid start context"),
            uc_core::membership::AdmissionShortInvitationCode::from_bytes(
                input.invitation_code.as_str().as_bytes().to_vec(),
            )
            .expect("valid short code"),
        ))
    }
}

#[async_trait]
impl ResolveJoinerInvitationPort for FixedJoinerInvitationResolver {
    async fn resolve_once(
        &self,
        short_code: &uc_core::membership::AdmissionShortInvitationCode,
    ) -> Result<uc_core::pairing::invitation::FullInvitation, ResolveJoinerInvitationError> {
        self.events
            .lock()
            .expect("events are available")
            .push(ProtocolEvent::JoinerInvitationResolutionRequested);
        let code = std::str::from_utf8(short_code.as_bytes()).map_err(|error| {
            ResolveJoinerInvitationError::unavailable(anyhow::Error::new(error))
        })?;
        if code == "short-fail" {
            return Err(ResolveJoinerInvitationError::unavailable(anyhow::anyhow!(
                "simulated consumed short code"
            )));
        }
        uc_core::pairing::invitation::FullInvitation::new(format!("ucspace1_resolved-{code}"))
            .map_err(|error| ResolveJoinerInvitationError::unavailable(anyhow::Error::new(error)))
    }
}

pub(super) fn authenticated_join_request() -> AuthenticatedSpaceAdmissionMessage {
    let admission_id = SpaceAdmissionId::from_bytes([0x51; 32]).expect("valid admission id");
    let envelope = join_request_envelope(admission_id, [0x52; 32]);
    AuthenticatedSpaceAdmissionMessage::new(
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0x53; 32]).expect("valid local peer"),
            AdmissionChannelPeerId::from_bytes([0x54; 32]).expect("valid remote peer"),
        )
        .expect("distinct peers"),
        envelope,
        [0x55; 32],
        Some(
            AdmissionContinuationCredential::from_bytes(vec![0x56; 64])
                .expect("valid continuation credential"),
        ),
    )
    .expect("valid authenticated message")
}

pub(super) fn authenticated_prepared(
    candidate: &SpaceAdmissionEnvelopeV1,
) -> AuthenticatedSpaceAdmissionMessage {
    authenticated_prepared_with_peers(candidate, 0x53, 0x54)
}

pub(super) fn authenticated_applied(
    commit: &SpaceAdmissionEnvelopeV1,
) -> AuthenticatedSpaceAdmissionMessage {
    let SpaceAdmissionBodyV1::Commit(commit_body) = commit.body() else {
        panic!("fixture Commit body is required");
    };
    let candidate = commit_body.exact_candidate();
    let applied = SpaceAdmissionEnvelopeV1::new(
        commit.header().admission_id(),
        AdmissionRole::Joiner,
        2,
        AdmissionMessageId::from_bytes([0xa5; 32]).expect("valid Applied message id"),
        Some(commit.header().message_id()),
        SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(AdmissionActivationReceipt::new(
            1,
            *commit.header().admission_id().as_bytes(),
            candidate.candidate_event().event_id(),
            [0xa6; 32],
            candidate.security_commitment().security_commitment_id,
            MemberInstanceId::from_bytes([0xa7; 32]),
            vec![0xa8; 64],
        ))),
    )
    .expect("valid Applied request");
    AuthenticatedSpaceAdmissionMessage::new(
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0x53; 32]).expect("valid local peer"),
            AdmissionChannelPeerId::from_bytes([0x54; 32]).expect("valid remote peer"),
        )
        .expect("distinct peers"),
        applied,
        [0xa9; 32],
        None,
    )
    .expect("valid authenticated Applied")
}

pub(super) fn authenticated_complete_ack(
    complete: &SpaceAdmissionEnvelopeV1,
) -> AuthenticatedSpaceAdmissionMessage {
    let complete_ack = SpaceAdmissionEnvelopeV1::new(
        complete.header().admission_id(),
        AdmissionRole::Joiner,
        3,
        AdmissionMessageId::from_bytes([0xc2; 32]).expect("valid CompleteAck message id"),
        Some(complete.header().message_id()),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([0xc3; 32]).expect("valid completion digest"),
        ),
    )
    .expect("valid CompleteAck request");
    AuthenticatedSpaceAdmissionMessage::new(
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0x53; 32]).expect("valid local peer"),
            AdmissionChannelPeerId::from_bytes([0x54; 32]).expect("valid remote peer"),
        )
        .expect("distinct peers"),
        complete_ack,
        [0xc4; 32],
        None,
    )
    .expect("valid authenticated CompleteAck")
}

pub(super) fn authenticated_prepared_with_peers(
    candidate: &SpaceAdmissionEnvelopeV1,
    local_peer: u8,
    remote_peer: u8,
) -> AuthenticatedSpaceAdmissionMessage {
    let SpaceAdmissionBodyV1::Candidate(candidate_body) = candidate.body() else {
        panic!("fixture Candidate body is required");
    };
    let prepared = SpaceAdmissionEnvelopeV1::new(
        candidate.header().admission_id(),
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0x91; 32]).expect("valid Prepared message id"),
        Some(candidate.header().message_id()),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(PreparedAdmissionProofV1::new(
            *candidate.header().admission_id().as_bytes(),
            "lineage".to_owned(),
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0x92; 32],
            },
            candidate_body.candidate_event().event_id(),
            candidate_body.candidate_event().resulting_members_digest,
            candidate_body.security_commitment().security_commitment_id,
            MemberInstanceId::from_bytes([0x93; 32]),
            MembershipCredential::new(1, vec![0x94; 32]).credential_id,
            vec![0x95; 64],
        ))),
    )
    .expect("valid Prepared request");
    AuthenticatedSpaceAdmissionMessage::new(
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([local_peer; 32]).expect("valid local peer"),
            AdmissionChannelPeerId::from_bytes([remote_peer; 32]).expect("valid remote peer"),
        )
        .expect("distinct peers"),
        prepared,
        [0x96; 32],
        None,
    )
    .expect("valid authenticated Prepared")
}

fn join_request_envelope(
    admission_id: SpaceAdmissionId,
    message_id: [u8; 32],
) -> SpaceAdmissionEnvelopeV1 {
    let device_id = DeviceId::new("joining-device");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x58; 32]);
    let signature = vec![0x5b; 64];
    let request = AdmissionJoinRequestV1::new(
        InvitationId::from_bytes([0x57; 32]).expect("valid invitation id"),
        device_id.clone(),
        join_request_identity_facts(device_id, &credential, signature.clone()),
        credential,
        AdmissionKeyPackage::from_bytes(vec![0x59; 48]).expect("valid key package"),
        AdmissionRecoveryPublicKey::from_bytes([0x5a; 32]).expect("valid recovery public key"),
        AdmissionIdentitySignature::from_bytes(signature).expect("valid identity signature"),
        UnreadableHistoryPolicy::Discard,
    )
    .expect("valid join request");
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        AdmissionMessageId::from_bytes(message_id).expect("valid message id"),
        None,
        SpaceAdmissionBodyV1::JoinRequest(request),
    )
    .expect("valid join request envelope")
}

fn candidate_body_fixture() -> AdmissionCandidateV1 {
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x61; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x62; 32]);
    let joiner_device = DeviceId::new("candidate-joiner");
    let joiner_member = joiner_credential.member_instance_id(&joiner_device);
    let admission = MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance: joiner_member,
            device_id: joiner_device,
            device_name: "candidate-joiner".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("valid fingerprint"),
            transport_public_key: vec![0x63; 32],
            transport_address_blob: vec![0x64; 16],
            identity_signature: vec![0x65; 64],
        },
        membership_credential: joiner_credential,
        resume_public_key_digest: [0x66; 32],
        security_commitment_id: [0x67; 32],
    };
    let candidate_event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "lineage".to_owned(),
        None,
        0,
        [0x68; 16],
        MemberInstanceId::from_bytes([0x69; 32]),
        sponsor_credential.credential_id,
        ED25519_SIGNATURE_ALGORITHM_V1,
        MembershipOperationV2::AddDevice { admission },
        [0x6a; 32],
        [0x6b; 32],
        vec![0x6c],
        Some([0x6d; 32]),
        vec![0x6e; 64],
    );
    let base_position = BaseMembershipHistoryPosition {
        event_id: None,
        depth: 0,
        history_digest: [0x6f; 32],
    };
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        "lineage".to_owned(),
        vec![0x70; 16],
        [0x71; 32],
        base_position,
        [0x72; 32],
        1,
        0,
        1,
        [0x73; 32],
        [0x74; 32],
        [0x75; 32],
        [0x76; 32],
        [0x77; 32],
    )
    .expect("valid security commitment");
    AdmissionCandidateV1::new(
        AdmissionSignedMembershipHistory::from_bytes(vec![0x78; 64]).expect("valid history"),
        candidate_event,
        commitment,
        AdmissionMlsCommit::from_bytes(vec![0x79; 64]).expect("valid MLS commit"),
        AdmissionMlsWelcome::from_bytes(vec![0x7a; 64]).expect("valid MLS welcome"),
        uc_core::membership::AdmissionContinuationRoute::from_bytes(vec![0x7b; 32])
            .expect("valid continuation route"),
    )
    .expect("valid candidate")
}

#[async_trait]
impl JoinerStartMaterialPort for FixedJoinerStartMaterial {
    async fn create(
        &self,
        input: &crate::space::admission::JoinSpaceInput,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        let admission_id = SpaceAdmissionId::from_bytes([0x11; 32]).expect("valid admission id");
        let join_id = JoinId::from_bytes([0x12; 16]).expect("valid join id");
        fixed_joiner_start_material(admission_id, join_id, input.preserve_unreadable_history)
    }

    async fn create_resolved(
        &self,
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        _invitation: &FullInvitation,
        _start_context: &AdmissionJoinerStartContext,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        fixed_joiner_start_material(admission_id, join_id, false)
    }
}

fn fixed_joiner_start_material(
    admission_id: SpaceAdmissionId,
    join_id: JoinId,
    preserve_unreadable_history: bool,
) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
    let device_id = DeviceId::new("joining-device");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x14; 32]);
    let signature = vec![0x17; 64];

    let request = AdmissionJoinRequestV1::new(
        InvitationId::from_bytes([0x13; 32]).expect("valid invitation id"),
        device_id.clone(),
        join_request_identity_facts(device_id, &credential, signature.clone()),
        credential,
        AdmissionKeyPackage::from_bytes(vec![0x15; 48]).expect("valid key package"),
        AdmissionRecoveryPublicKey::from_bytes([0x16; 32]).expect("valid recovery public key"),
        AdmissionIdentitySignature::from_bytes(signature).expect("valid identity signature"),
        if preserve_unreadable_history {
            UnreadableHistoryPolicy::Preserve
        } else {
            UnreadableHistoryPolicy::Discard
        },
    )
    .expect("valid join request");
    let join_request = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        AdmissionMessageId::from_bytes([0x18; 32]).expect("valid message id"),
        None,
        SpaceAdmissionBodyV1::JoinRequest(request),
    )
    .expect("valid join request envelope");

    Ok(JoinerStartMaterial::new(
        admission_id,
        join_id,
        SpaceAdmissionRoute::from_bytes(vec![0x19; 32]).expect("valid route"),
        join_request,
        AdmissionJoinerPrivateState::from_bytes(vec![0x1b; 64])
            .expect("valid Joiner private state"),
        AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0x1a; 64])
            .expect("valid password material"),
    ))
}
