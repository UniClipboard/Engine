use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::*;
use uc_core::security::IdentityFingerprint;

use super::admission::*;
use super::application::{SpaceApplication, SpaceRuntimeAdapters};
use super::membership::*;

struct MemoryLedger(Mutex<LoadedMembershipLedger>);

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
        transport_public_key: vec![0x88; 32],
        transport_address_blob: vec![0x89; 32],
        identity_signature: signature,
    }
}

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedger {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut loaded = self.0.lock().unwrap();
        if loaded.revision != mutation.expected_revision {
            return Err(MembershipLedgerError::Conflict);
        }
        *loaded = mutation.replacement;
        Ok(loaded.clone())
    }
}

#[derive(Default)]
struct PassivePorts {
    join_commits: AtomicUsize,
}

impl HistoricalMembershipSignatureVerifier for PassivePorts {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

#[async_trait]
impl GroupUpdateDispatchPort for PassivePorts {
    async fn dispatch_group_update(
        &self,
        _update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError> {
        Ok(())
    }
}

#[async_trait]
impl GroupRevocationPort for PassivePorts {
    async fn revoke_group_member(
        &self,
        _: &DeviceId,
        _: &[DeviceId],
        _: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        unreachable!()
    }
    async fn acknowledge_group_update(
        &self,
        _: &RevocationId,
        _: &DeviceId,
        _: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        unreachable!()
    }
    async fn apply_group_epoch_update(&self, _: &[u8]) -> Result<GroupEpoch, KeyEpochError> {
        unreachable!()
    }
    async fn pending_group_updates(
        &self,
        _: &RevocationId,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        unreachable!()
    }
    async fn query_group_revocation(
        &self,
        _: &RevocationId,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        unreachable!()
    }
    async fn resume_group_revocations(
        &self,
        _: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
        Ok(Vec::new())
    }
    async fn pending_space_group_updates(&self) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        Ok(Vec::new())
    }
    async fn acknowledge_space_group_update(&self, _: &str, _: i64) -> Result<bool, KeyEpochError> {
        Ok(false)
    }
}

#[async_trait]
impl CurrentMemberSignaturePort for PassivePorts {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        unreachable!()
    }

    async fn current_member_instance(
        &self,
        _device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        unreachable!()
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        unreachable!()
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        unreachable!()
    }
}

#[async_trait]
impl CurrentMembershipIdentityPort for PassivePorts {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
        unreachable!()
    }
}

#[async_trait]
impl CurrentMembershipAnnouncementPort for PassivePorts {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        unreachable!()
    }

    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        unreachable!()
    }
}

impl uc_core::ports::DeviceIdentityPort for PassivePorts {
    fn current_device_id(&self) -> DeviceId {
        DeviceId::new("local")
    }
}

impl uc_core::ports::ClockPort for PassivePorts {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

#[async_trait]
impl uc_core::ports::SettingsPort for PassivePorts {
    async fn load(&self) -> anyhow::Result<uc_core::settings::model::Settings> {
        Ok(uc_core::settings::model::Settings::default())
    }

    async fn save(&self, _settings: &uc_core::settings::model::Settings) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl GroupBootstrapPort for PassivePorts {
    async fn bootstrap_legacy_space(
        &self,
        _sponsor: &DeviceId,
        _retained_members: &[DeviceId],
        _now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        unreachable!()
    }

    async fn acknowledge_legacy_readmission(
        &self,
        _bootstrap_id: &BootstrapId,
        _member: &DeviceId,
        _now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        unreachable!()
    }

    async fn withdraw_legacy_readmission(
        &self,
        _bootstrap_id: &BootstrapId,
        _member: &DeviceId,
        _now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        unreachable!()
    }

    async fn query_legacy_bootstrap(
        &self,
        _bootstrap_id: &BootstrapId,
    ) -> Result<Option<GroupBootstrapResult>, BootstrapError> {
        unreachable!()
    }

    async fn resume_legacy_bootstraps(
        &self,
        _now_ms: i64,
    ) -> Result<Vec<GroupBootstrapResult>, BootstrapError> {
        unreachable!()
    }
}

#[async_trait]
impl JoinerStartMaterialPort for PassivePorts {
    async fn create(
        &self,
        input: &JoinSpaceInput,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        let admission_id = SpaceAdmissionId::from_bytes([0x81; 32]).expect("valid admission id");
        let join_id = JoinId::from_bytes([0x82; 16]).expect("valid join id");
        let device_id = DeviceId::new("joining-device");
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x84; 32]);
        let signature = vec![0x87; 64];
        let request = AdmissionJoinRequestV1::new(
            InvitationId::from_bytes([0x83; 32]).expect("valid invitation id"),
            device_id.clone(),
            join_request_identity_facts(device_id, &credential, signature.clone()),
            credential,
            AdmissionKeyPackage::from_bytes(vec![0x85; 48]).expect("valid key package"),
            AdmissionRecoveryPublicKey::from_bytes([0x86; 32]).expect("valid recovery public key"),
            AdmissionIdentitySignature::from_bytes(signature).expect("valid identity signature"),
            if input.preserve_unreadable_history {
                UnreadableHistoryPolicy::Preserve
            } else {
                UnreadableHistoryPolicy::Discard
            },
        )
        .expect("valid join request");
        let request = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            AdmissionMessageId::from_bytes([0x88; 32]).expect("valid message id"),
            None,
            SpaceAdmissionBodyV1::JoinRequest(request),
        )
        .expect("valid join request envelope");
        Ok(JoinerStartMaterial::new(
            admission_id,
            join_id,
            SpaceAdmissionRoute::from_bytes(vec![0x89; 32]).expect("valid route"),
            request,
            AdmissionJoinerPrivateState::from_bytes(vec![0x8d; 64])
                .expect("valid Joiner private state"),
            AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0x8a; 64])
                .expect("valid password material"),
        ))
    }
}

#[async_trait]
impl PrepareJoinerInvitationPort for PassivePorts {
    async fn prepare(
        &self,
        _input: &JoinSpaceInput,
    ) -> Result<PreparedJoinerInvitation, PrepareJoinerInvitationError> {
        Ok(PreparedJoinerInvitation::Full)
    }
}

#[async_trait]
impl ResolveJoinerInvitationPort for PassivePorts {
    async fn resolve_once(
        &self,
        _short_code: &AdmissionShortInvitationCode,
    ) -> Result<uc_core::pairing::invitation::FullInvitation, ResolveJoinerInvitationError> {
        unreachable!()
    }
}

#[async_trait]
impl JoinerStartStatePort for PassivePorts {
    async fn load(&self) -> Result<LoadedJoinerStartState, JoinerStartStateError> {
        Ok(LoadedJoinerStartState::new(
            11,
            AdmissionSourceSnapshot::from_bytes(vec![0x8b; 32]).expect("valid source snapshot"),
            None,
            false,
            SpaceAdmissionCommitToken::from_bytes([0x8c; 32]).expect("valid commit token"),
        ))
    }

    async fn commit(
        &self,
        _token: SpaceAdmissionCommitToken,
        _mutation: JoinerStartMutation,
    ) -> Result<(), JoinerStartStateError> {
        self.join_commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl CurrentJoinAdmissionStatePort for PassivePorts {
    async fn load(
        &self,
        _join_id: JoinId,
    ) -> Result<Option<LoadedCurrentJoin>, JoinerCancellationStateError> {
        Ok(None)
    }

    async fn commit(
        &self,
        _token: JoinerCancellationCommitToken,
        _mutation: JoinerCancellationMutation,
    ) -> Result<(), JoinerCancellationStateError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinerCancellationPort for PassivePorts {
    async fn prepare(&self) -> Result<JoinerCancellationMaterial, JoinerCancellationMaterialError> {
        Ok(JoinerCancellationMaterial::new(
            AdmissionMessageId::from_bytes([0x8d; 32]).expect("valid cancellation message id"),
            AdmissionRetryState::new(0, 0).expect("valid cancellation retry state"),
        ))
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for PassivePorts {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }
}

#[async_trait]
impl PendingAdmissionRecoveryStatePort for PassivePorts {
    async fn load(
        &self,
        _trigger: AdmissionRecoveryTrigger,
    ) -> Result<Vec<LoadedPendingAdmission>, PendingAdmissionRecoveryStateError> {
        Ok(Vec::new())
    }

    async fn commit(
        &self,
        _token: AdmissionRecoveryCommitToken,
        _transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        unreachable!()
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for PassivePorts {
    async fn establish_initial(
        &self,
        _admission_id: SpaceAdmissionId,
        _route: &SpaceAdmissionRoute,
        _encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        Err(SpaceAdmissionTransportError::Deferred)
    }

    async fn resume(
        &self,
        _admission_id: SpaceAdmissionId,
        _route: &SpaceAdmissionRoute,
        _peer_binding: AdmissionPeerBinding,
        _continuation_credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        Err(SpaceAdmissionTransportError::Deferred)
    }
}

#[async_trait]
impl SponsorAdmissionStatePort for PassivePorts {
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
impl PrepareSponsorCandidatePort for PassivePorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSponsorCommitPort for PassivePorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: SponsorCommitPreparation<'_>,
        _prepared: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorCommit, PrepareSponsorCommitError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSponsorCompletePort for PassivePorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: SponsorCompletePreparation<'_>,
        _applied: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorComplete, PrepareSponsorCompleteError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSponsorSettledPort for PassivePorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: SponsorSettlementPreparation<'_>,
        _complete_ack: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorSettled, PrepareSponsorSettledError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinerCandidatePort for PassivePorts {
    async fn prepare(
        &self,
        _preparation: JoinerCandidatePreparation<'_>,
        _candidate: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinerAppliedPort for PassivePorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: JoinerAppliedPreparation<'_>,
    ) -> Result<PreparedJoinerAppliedMaterial, PrepareJoinerAppliedError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinerActivationPort for PassivePorts {
    async fn prepare(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: JoinerCompletePreparation<'_>,
        _complete: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerActivation, PrepareJoinerActivationError> {
        unreachable!()
    }
}

#[async_trait]
impl JoinerActivationStatePort for PassivePorts {
    async fn load(&self) -> Result<Option<LoadedJoinerActivation>, JoinerActivationStateError> {
        Ok(None)
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
impl ExecuteJoinerActivationPort for PassivePorts {
    async fn execute(
        &self,
        _admission_id: SpaceAdmissionId,
        _preparation: JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError> {
        unreachable!()
    }
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for PassivePorts {
    async fn load(
        &self,
        _device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for PassivePorts {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        _message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        unreachable!()
    }
}

#[async_trait]
impl AdmissionSpaceTransitionPort for PassivePorts {
    async fn prepare_if_needed(
        &self,
        _input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        unreachable!()
    }

    async fn advance(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        unreachable!()
    }

    async fn discard_pre_activation(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        unreachable!()
    }
}

#[async_trait]
impl ApplyMembershipMemberFactsPort for PassivePorts {
    async fn apply_member_facts(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        unreachable!()
    }
}

#[async_trait]
impl ApplyMembershipSecurityPort for PassivePorts {
    async fn apply_membership_security(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        unreachable!()
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for PassivePorts {
    async fn activate_membership_effect(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        unreachable!()
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for PassivePorts {
    async fn deliver_restricted_membership(
        &self,
        _peer: &DeviceId,
        _delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        unreachable!()
    }
}

#[async_trait]
impl CleanupLegacyMembershipDataPort for PassivePorts {
    async fn cleanup_legacy_membership_data(&self) -> MembershipMaintenanceStepOutcome {
        MembershipMaintenanceStepOutcome::Completed
    }
}

impl MembershipNetworkActivityPort for PassivePorts {
    fn pause_network_work(&self) {}
    fn resume_network_work(&self) {}
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for PassivePorts {
    async fn request_membership_branch_group_info(
        &self,
        _request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        Err(MembershipBranchRecoveryChannelError::Unavailable {
            source: anyhow::anyhow!("passive recovery source"),
        })
    }

    async fn submit_membership_branch_external_commit(
        &self,
        _request: MembershipBranchRecoveryCommit,
    ) -> Result<
        uc_core::membership::MembershipBranchRecoveryPackageV1,
        MembershipBranchRecoveryChannelError,
    > {
        Err(MembershipBranchRecoveryChannelError::Unavailable {
            source: anyhow::anyhow!("passive recovery source"),
        })
    }
}

#[async_trait]
impl PrepareMembershipBranchRecoveryRecipientPort for PassivePorts {
    async fn prepare_membership_branch_recovery_recipient(
        &self,
        _group_info: Vec<u8>,
    ) -> Result<
        PreparedMembershipBranchRecoveryRecipient,
        PrepareMembershipBranchRecoveryRecipientError,
    > {
        Err(PrepareMembershipBranchRecoveryRecipientError::Unavailable {
            source: anyhow::anyhow!("passive recovery recipient"),
        })
    }
}

#[async_trait]
impl PrepareMembershipBranchTransitionPort for PassivePorts {
    async fn prepare_membership_branch_transition(
        &self,
        _input: PrepareMembershipBranchTransitionInput,
    ) -> Result<
        uc_core::membership::MembershipBranchTransitionV1,
        PrepareMembershipBranchTransitionError,
    > {
        Err(PrepareMembershipBranchTransitionError::Unavailable {
            source: anyhow::anyhow!("passive transition preparation"),
        })
    }
}

#[async_trait]
impl AdvanceMembershipBranchTransitionPort for PassivePorts {
    async fn advance_membership_branch_transition(
        &self,
        _input: AdvanceMembershipBranchTransitionInput,
    ) -> Result<
        uc_core::membership::MembershipBranchTransitionV1,
        AdvanceMembershipBranchTransitionError,
    > {
        Err(AdvanceMembershipBranchTransitionError::Unavailable {
            source: anyhow::anyhow!("passive transition execution"),
        })
    }
}

#[async_trait]
impl PrepareMembershipBranchRecoveryMaterialPort for PassivePorts {
    async fn export_membership_branch_recovery_group_info(
        &self,
    ) -> Result<Vec<u8>, PrepareMembershipBranchRecoveryMaterialError> {
        Err(PrepareMembershipBranchRecoveryMaterialError::Unavailable {
            source: anyhow::anyhow!("passive recovery group info"),
        })
    }

    async fn prepare_membership_branch_recovery_material(
        &self,
        _input: PrepareMembershipBranchRecoveryMaterialInput,
    ) -> Result<
        PreparedMembershipBranchRecoveryMaterial,
        PrepareMembershipBranchRecoveryMaterialError,
    > {
        Err(PrepareMembershipBranchRecoveryMaterialError::Unavailable {
            source: anyhow::anyhow!("passive recovery material"),
        })
    }

    async fn commit_membership_branch_recovery_material(
        &self,
        _target_staged_space_material: Vec<u8>,
    ) -> Result<(), PrepareMembershipBranchRecoveryMaterialError> {
        Err(PrepareMembershipBranchRecoveryMaterialError::Unavailable {
            source: anyhow::anyhow!("passive recovery material"),
        })
    }
}

#[async_trait]
impl ActivateSponsorAdmissionPort for PassivePorts {
    async fn activate(
        &self,
        _activated_security: &uc_core::membership::AdmissionActivatedSecurityState,
    ) -> Result<(), ActivateSponsorAdmissionError> {
        Ok(())
    }
}

#[async_trait]
impl ResolveRePairingPort for PassivePorts {
    async fn resolve_after_successful_pairing(&self) -> Result<(), RePairingStateError> {
        Ok(())
    }
}

#[tokio::test]
async fn complete_application_exposes_endpoints_before_runtime_starts() {
    let repository = Arc::new(MemoryLedger(Mutex::new(
        LoadedMembershipLedger::no_current_space(),
    )));
    let passive = Arc::new(PassivePorts::default());
    let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
    let mut application = SpaceApplication::build_for_test(
        SpaceRuntimeAdapters {
            load_membership_ledger: repository.clone(),
            commit_membership_ledger: repository,
            historical_membership_signatures: passive.clone(),
            current_member_signatures: passive.clone(),
            membership_identity: passive.clone(),
            membership_announcement: passive.clone(),
            prepare_joiner_invitation: passive.clone(),
            resolve_joiner_invitation: passive.clone(),
            joiner_start_material: passive.clone(),
            joiner_start_state: passive.clone(),
            current_join_admission_state: passive.clone(),
            prepare_joiner_cancellation: passive.clone(),
            pending_admission_recovery_state: passive.clone(),
            space_admission_transport: passive.clone(),
            sponsor_admission_state: passive.clone(),
            prepare_sponsor_candidate: passive.clone(),
            prepare_sponsor_commit: passive.clone(),
            prepare_sponsor_complete: passive.clone(),
            activate_sponsor_admission: passive.clone(),
            prepare_sponsor_settled: passive.clone(),
            prepare_joiner_candidate: passive.clone(),
            prepare_joiner_applied: passive.clone(),
            prepare_joiner_activation: passive.clone(),
            joiner_activation_state: passive.clone(),
            execute_joiner_activation: passive.clone(),
            device_trust_observations: passive.clone(),
            current_join_status: passive.clone(),
            membership_history_transport: passive.clone(),
            membership_branch_recovery_channel: passive.clone(),
            membership_branch_recovery_recipient: passive.clone(),
            membership_branch_transition: passive.clone(),
            membership_branch_transition_executor: passive.clone(),
            membership_branch_recovery_material: passive.clone(),
            apply_membership_member_facts: passive.clone(),
            apply_membership_security: passive.clone(),
            activate_membership_effect: passive.clone(),
            restricted_membership_delivery: passive.clone(),
            group_update_store: passive.clone(),
            group_update_dispatch: passive.clone(),
            cleanup_legacy_membership_data: passive.clone(),
            membership_network_activity: passive.clone(),
        },
        passive.clone(),
        passive.clone(),
        passive.clone(),
        passive.clone(),
        presence_rx,
        passive.clone(),
    );

    let _ = application.membership_history_endpoint();
    let _ = application.space_admission_endpoint();
    let _ = application.current_scope();

    let joined = application
        .space_admission()
        .start_join(JoinSpaceInput {
            invitation_code: uc_core::pairing::InvitationCode::new("join-code"),
            device_name: Some("New Device".to_owned()),
            passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
            preserve_unreadable_history: false,
        })
        .await
        .expect("new protocol JoinSpace should return after saving Pending");
    assert!(matches!(joined.status, CurrentJoinStatus::Pending { .. }));
    assert_eq!(passive.join_commits.load(Ordering::SeqCst), 1);

    assert!(application.start_runtime());
    assert!(!application.start_runtime());
    application.shutdown().await;
}
