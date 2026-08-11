//! WorkspaceConvergence owner tests (ADR-016 flow semantics).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, CurrentMembershipAnnouncementMaterial,
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentity, CurrentMembershipIdentityError,
    CurrentMembershipIdentityPort, MemberInstanceId, MemberRepositoryPort,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalCausalProof,
    RemovalCausalProofMember, RemovalExchangeEndpointPort, RemovalExchangeMessage,
    RemovalIntentContent, RemovalIntentVerificationError, RemovalIntentVerificationPort,
    RemovalLateAcceptance, RemovalLateRejectionReason, RemovalLateSubmission,
    RemovalLateSubmissionEndpointPort, RemovalNotice, RemovalNoticeAcceptance,
    RemovalNoticeEndpointPort, RemovalRecoveryError, RemovalRecoveryPort, RemovalViewMember,
    RemovalViewSnapshot, SignedRemovalIntent, WorkspaceChange, WorkspaceChangeKind,
    WorkspaceConvergenceEvent, WorkspaceConvergenceRepositoryError,
    WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState, WorkspacePhase,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort};

use crate::space::convergence::{
    WorkspaceConvergence, WorkspaceConvergenceDeps, WorkspaceConvergenceError,
};

const SPACE: &str = "test-workspace";

#[derive(Clone, Default)]
struct UnusedExchange {
    sent: Arc<Mutex<Vec<uc_core::membership::RemovalExchangeMessage>>>,
}

#[async_trait]
impl uc_core::membership::RemovalExchangePort for UnusedExchange {
    async fn exchange(
        &self,
        _recipient: &DeviceId,
        message: uc_core::membership::RemovalExchangeMessage,
    ) -> Result<
        uc_core::membership::RemovalExchangeMessage,
        uc_core::membership::RemovalExchangeError,
    > {
        self.sent.lock().unwrap().push(message);
        Ok(uc_core::membership::RemovalExchangeMessage::IntentAck(
            uc_core::membership::RemovalIntentId::from_bytes([0; 32]),
        ))
    }
}

#[derive(Clone, Default)]
struct UnusedLate;

#[async_trait]
impl uc_core::membership::RemovalLateSubmissionPort for UnusedLate {
    async fn submit_late(
        &self,
        _recipient: &DeviceId,
        _submission: uc_core::membership::RemovalLateSubmission,
    ) -> Result<
        uc_core::membership::RemovalLateAcceptance,
        uc_core::membership::RemovalLateSubmissionTransportError,
    > {
        Ok(uc_core::membership::RemovalLateAcceptance::AlreadyKnown {
            intent_id: uc_core::membership::RemovalIntentId::from_bytes([0; 32]),
        })
    }
}

#[derive(Clone, Default)]
struct UnusedNotice;

#[async_trait]
impl uc_core::membership::RemovalNoticePort for UnusedNotice {
    async fn send_notice(
        &self,
        _recipient: &DeviceId,
        _notice: uc_core::membership::RemovalNotice,
    ) -> Result<
        uc_core::membership::RemovalNoticeAcceptance,
        uc_core::membership::RemovalNoticeTransportError,
    > {
        Ok(uc_core::membership::RemovalNoticeAcceptance::Accepted {
            intent_id: uc_core::membership::RemovalIntentId::from_bytes([0; 32]),
        })
    }
}

#[derive(Clone, Default)]
struct RejectingNoticeVerification;

#[async_trait]
impl uc_core::membership::RemovalNoticeVerificationPort for RejectingNoticeVerification {
    async fn verify_notice_signature(
        &self,
        _notice: &uc_core::membership::RemovalNotice,
        _issuer_public_key: &[u8],
    ) -> Result<(), uc_core::membership::RemovalNoticeVerificationError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct MemoryWorkspaceRepository {
    state: Arc<Mutex<Option<WorkspaceConvergenceState>>>,
    failure: Arc<Mutex<Option<WorkspaceConvergenceRepositoryError>>>,
}

#[async_trait]
impl WorkspaceConvergenceRepositoryPort for MemoryWorkspaceRepository {
    async fn save_state(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }

    async fn load_state(
        &self,
    ) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
        Ok(self.state.lock().unwrap().clone())
    }
}

#[derive(Clone)]
struct FixedMembershipIdentity {
    space: SpaceId,
}

#[async_trait]
impl CurrentMembershipIdentityPort for FixedMembershipIdentity {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
        Ok(CurrentMembershipIdentity {
            space_id: self.space.clone(),
            device_id: DeviceId::new("device-a"),
            device_name: "a".to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
        })
    }
}

struct FixedAnnouncementMaterial;

#[async_trait]
impl CurrentMembershipAnnouncementPort for FixedAnnouncementMaterial {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: SpaceId::from_str(SPACE),
            device_id: DeviceId::new("device-a"),
            device_name: "a".into(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1; 32],
            transport_address_blob: vec![2],
        })
    }
    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AcceptingVerifier;

#[async_trait]
impl RemovalIntentVerificationPort for AcceptingVerifier {
    async fn verify_intent(
        &self,
        _intent: &SignedRemovalIntent,
    ) -> Result<(), RemovalIntentVerificationError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct FixedSigner;

#[async_trait]
impl CurrentMemberSignaturePort for FixedSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(b"signature".to_vec())
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        Ok(true)
    }
}

#[derive(Clone)]
struct FakeRecovery {
    members: Arc<Mutex<Vec<(DeviceId, MemberInstanceId)>>>,
    own_device: DeviceId,
}

impl FakeRecovery {
    fn new(own_device: DeviceId, members: Vec<(DeviceId, MemberInstanceId)>) -> Self {
        Self {
            members: Arc::new(Mutex::new(members)),
            own_device,
        }
    }
}

#[async_trait]
impl RemovalRecoveryPort for FakeRecovery {
    async fn current_view(&self) -> Result<RemovalViewSnapshot, RemovalRecoveryError> {
        let members = self
            .members
            .lock()
            .unwrap()
            .iter()
            .map(|(device_id, instance)| RemovalViewMember {
                device_id: *device_id,
                instance: *instance,
                signing_public_key: instance.as_bytes().to_vec(),
            })
            .collect::<Vec<_>>();
        let causal_proof = RemovalCausalProof::new(
            1,
            members
                .iter()
                .map(|member| RemovalCausalProofMember {
                    device_id: member.device_id,
                    instance: member.instance,
                    signing_public_key: member.signing_public_key.clone(),
                })
                .collect(),
        );
        Ok(RemovalViewSnapshot {
            epoch: 1,
            members,
            causal_proof,
        })
    }

    async fn own_instance(&self) -> Result<Option<MemberInstanceId>, RemovalRecoveryError> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .find(|(device_id, _)| *device_id == self.own_device)
            .map(|(_, instance)| *instance))
    }
}

#[derive(Clone, Default)]
struct UnusedSecurityUpdates;

#[async_trait]
impl MembershipSecurityUpdatePort for UnusedSecurityUpdates {
    async fn current_state(
        &self,
    ) -> Result<uc_core::membership::MembershipSecurityState, MembershipSecurityUpdateError> {
        Ok(uc_core::membership::MembershipSecurityState {
            space_id: SpaceId::from_str(SPACE),
            group_epoch: 0,
        })
    }

    async fn apply_group_epoch_update(
        &self,
        _payload: &[u8],
    ) -> Result<u64, MembershipSecurityUpdateError> {
        Ok(0)
    }
}

#[derive(Clone, Default)]
struct UnusedClock;

#[async_trait]
impl ClockPort for UnusedClock {
    fn now_ms(&self) -> i64 {
        1_000
    }
}

#[derive(Clone, Default)]
struct UnusedDeviceIdentity;

#[async_trait]
impl DeviceIdentityPort for UnusedDeviceIdentity {
    fn current_device_id(&self) -> DeviceId {
        DeviceId::new("device-a")
    }
}

struct Harness {
    owner: Arc<WorkspaceConvergence>,
    repository: MemoryWorkspaceRepository,
}

fn instance(byte: u8) -> MemberInstanceId {
    MemberInstanceId::from_bytes([byte; 32])
}

fn harness(own_device: &str, members: Vec<(DeviceId, MemberInstanceId)>) -> Harness {
    let repository = MemoryWorkspaceRepository::default();
    let owner =
        WorkspaceConvergence::new(test_deps(Arc::new(repository.clone()), own_device, members));
    Harness { owner, repository }
}

/// Build the full dependency set with no-op defaults for every port except
/// the repository and the recovery view. Shared with other test modules in
/// this crate (`pub(crate)` under `cfg(test)`).
pub(crate) fn test_deps(
    repository: Arc<dyn WorkspaceConvergenceRepositoryPort>,
    own_device: &str,
    members: Vec<(DeviceId, MemberInstanceId)>,
) -> WorkspaceConvergenceDeps {
    WorkspaceConvergenceDeps {
        repository,
        verification: Arc::new(AcceptingVerifier),
        recovery: Arc::new(FakeRecovery::new(DeviceId::new(own_device), members)),
        member_signatures: Arc::new(FixedSigner),
        member_repo: Arc::new(uc_application_test_member_repo()),
        membership_identity: Arc::new(FixedMembershipIdentity {
            space: SpaceId::from_str(SPACE),
        }),
        announcement_material: Arc::new(FixedAnnouncementMaterial),
        security_updates: Arc::new(UnusedSecurityUpdates),
        clock: Arc::new(UnusedClock),
        device_identity: Arc::new(UnusedDeviceIdentity),
        exchange: Arc::new(UnusedExchange::default()),
        late_submission: Arc::new(UnusedLate),
        notice: Arc::new(UnusedNotice),
        notice_verification: Arc::new(RejectingNoticeVerification),
        recovery_transport: Arc::new(UnusedRecoveryTransport),
        trusted_peer_repo: Arc::new(TestTrustedPeerRepo),
        peer_addr_repo: Arc::new(TestPeerAddrRepo),
        own_device: DeviceId::new(own_device),
    }
}

#[derive(Clone, Default)]
struct UnusedRecoveryTransport;

#[async_trait]
impl uc_core::membership::RecoveryTransportPort for UnusedRecoveryTransport {
    async fn exchange_recovery(
        &self,
        _recipient: &DeviceId,
        _binding: &uc_core::membership::RecoveryBinding,
        _message: uc_core::membership::RecoveryChannelMessage,
    ) -> Result<
        uc_core::membership::RecoveryChannelMessage,
        uc_core::membership::RecoveryTransportError,
    > {
        Err(uc_core::membership::RecoveryTransportError::Offline)
    }
}

struct TestTrustedPeerRepo;
#[async_trait]
impl uc_core::trusted_peer::TrustedPeerRepositoryPort for TestTrustedPeerRepo {
    async fn get(
        &self,
        _device_id: &DeviceId,
    ) -> Result<Option<uc_core::trusted_peer::TrustedPeer>, uc_core::trusted_peer::TrustedPeerError>
    {
        Ok(None)
    }
    async fn list(
        &self,
    ) -> Result<Vec<uc_core::trusted_peer::TrustedPeer>, uc_core::trusted_peer::TrustedPeerError>
    {
        Ok(Vec::new())
    }
    async fn save(
        &self,
        _peer: &uc_core::trusted_peer::TrustedPeer,
    ) -> Result<(), uc_core::trusted_peer::TrustedPeerError> {
        Ok(())
    }
    async fn remove(
        &self,
        _device_id: &DeviceId,
    ) -> Result<bool, uc_core::trusted_peer::TrustedPeerError> {
        Ok(true)
    }
}

struct TestPeerAddrRepo;
#[async_trait]
impl uc_core::ports::PeerAddressRepositoryPort for TestPeerAddrRepo {
    async fn get(
        &self,
        _device: &DeviceId,
    ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(None)
    }
    async fn upsert(
        &self,
        _record: &uc_core::ports::PeerAddressRecord,
    ) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }
    async fn list(
        &self,
    ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(Vec::new())
    }
    async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }
}

fn uc_application_test_member_repo() -> impl MemberRepositoryPort {
    struct Empty;
    #[async_trait]
    impl MemberRepositoryPort for Empty {
        async fn get(
            &self,
            _device_id: &DeviceId,
        ) -> Result<Option<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
        {
            Ok(None)
        }
        async fn list(
            &self,
        ) -> Result<Vec<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
        {
            Ok(Vec::new())
        }
        async fn save(
            &self,
            _member: &uc_core::membership::SpaceMember,
        ) -> Result<(), uc_core::membership::MembershipError> {
            Ok(())
        }
        async fn remove(
            &self,
            _device_id: &DeviceId,
        ) -> Result<bool, uc_core::membership::MembershipError> {
            Ok(true)
        }
    }
    Empty
}

fn admission_change_for(
    instance: MemberInstanceId,
    device: &DeviceId,
    lineage: &str,
) -> WorkspaceChange {
    WorkspaceChange {
        space_lineage: lineage.to_owned(),
        kind: WorkspaceChangeKind::Admission,
        previous_epoch: 0,
        next_epoch: 1,
        previous_digest: *uc_core::membership::WorkspaceDigest::from_bytes(
            Sha256::digest(b"uniclipboard-workspace-initial/v1").into(),
        )
        .as_bytes(),
        digest: [0; 32],
        security_updates: Vec::new(),
        admission: Some(uc_core::membership::AdmissionChangeFacts {
            member_instance: instance,
            device_id: *device,
            device_name: "device".to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1; 32],
            transport_address_blob: vec![2; 16],
            identity_signature: vec![3; 64],
        }),
        removal: None,
        created_at_ms: 1,
    }
}

#[tokio::test]
async fn removal_forms_a_continuous_change_and_publishes_a_snapshot() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );

    // Seed the chain with the two admissions so both members are effective.
    let repo = harness.repository.clone();
    {
        let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
        let first = admission_change_for(a, &DeviceId::new("device-a"), SPACE);
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(first), 1)
            .unwrap();
        let first_digest = uc_core::membership::compute_change_digest(&state.changes[0]);
        let mut second = admission_change_for(b, &DeviceId::new("device-b"), SPACE);
        second.previous_epoch = 1;
        second.next_epoch = 2;
        second.previous_digest = first_digest;
        second.digest = uc_core::membership::compute_change_digest(&second);
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(second), 1)
            .unwrap();
        repo.save_state(&state).await.unwrap();
    }

    let snapshot = harness
        .owner
        .submit_removal(&DeviceId::new("device-b"))
        .await
        .unwrap();
    assert_eq!(snapshot.effective_member_count, 1);
    assert_eq!(snapshot.change_count, 3);
    assert_eq!(snapshot.removal_intent_count, 1);
    assert_eq!(snapshot.phase, WorkspacePhase::Converging);

    let loaded = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        loaded.effective_members(),
        std::collections::BTreeSet::from([a])
    );
    assert_eq!(loaded.changes.len(), 3);
    assert!(!loaded.removed);
}

#[tokio::test]
async fn removing_an_unknown_or_self_target_fails_without_saving() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );

    assert!(matches!(
        harness
            .owner
            .submit_removal(&DeviceId::new("device-unknown"))
            .await,
        Err(WorkspaceConvergenceError::UnknownTarget)
    ));
    assert!(matches!(
        harness
            .owner
            .submit_removal(&DeviceId::new("device-a"))
            .await,
        Err(WorkspaceConvergenceError::SelfTarget)
    ));
    assert_eq!(
        harness.repository.load_state().await.unwrap(),
        None,
        "failed removal must not persist any state"
    );
}

#[tokio::test]
async fn queries_and_events_carry_the_complete_snapshot() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let mut events = harness.owner.subscribe();

    harness
        .owner
        .record_admission_change(admission_change_for(b, &DeviceId::new("device-b"), SPACE))
        .await
        .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.change_count, 1);
    assert_eq!(event.effective_member_count, 1);

    let queried = harness.owner.query().await.unwrap();
    assert_eq!(queried, event);
}

#[tokio::test]
async fn confirmation_of_the_current_digest_moves_towards_complete() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let repo = harness.repository.clone();
    {
        let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
        let first = admission_change_for(a, &DeviceId::new("device-a"), SPACE);
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(first), 1)
            .unwrap();
        let first_digest = uc_core::membership::compute_change_digest(&state.changes[0]);
        let mut second = admission_change_for(b, &DeviceId::new("device-b"), SPACE);
        second.previous_epoch = 1;
        second.next_epoch = 2;
        second.previous_digest = first_digest;
        second.digest = uc_core::membership::compute_change_digest(&second);
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(second), 1)
            .unwrap();
        repo.save_state(&state).await.unwrap();
    }

    let digest = *harness
        .owner
        .query()
        .await
        .unwrap()
        .convergence_digest
        .unwrap()
        .as_bytes();
    harness
        .owner
        .record_confirmation(uc_core::membership::WorkspaceConfirmation {
            member_instance: a,
            digest,
            signature: b"sig-a".to_vec(),
        })
        .await
        .unwrap();
    let snapshot = harness
        .owner
        .record_confirmation(uc_core::membership::WorkspaceConfirmation {
            member_instance: b,
            digest,
            signature: b"sig-b".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(snapshot.phase, WorkspacePhase::Complete);
    assert_eq!(snapshot.confirmed_member_count, 2);
}

fn signed_intent_for(
    lineage: &str,
    initiator: MemberInstanceId,
    target: MemberInstanceId,
) -> SignedRemovalIntent {
    let content = RemovalIntentContent {
        space_lineage: lineage.to_owned(),
        view_epoch: 1,
        view_members: vec![initiator, target],
        initiator,
        target,
    };
    content.validate().unwrap();
    SignedRemovalIntent::new(
        content,
        b"signature".to_vec(),
        RemovalCausalProof::new(
            1,
            vec![
                RemovalCausalProofMember {
                    device_id: DeviceId::new("device-a"),
                    instance: initiator,
                    signing_public_key: vec![1; 32],
                },
                RemovalCausalProofMember {
                    device_id: DeviceId::new("device-b"),
                    instance: target,
                    signing_public_key: vec![2; 32],
                },
            ],
        ),
    )
}

async fn seeded_two_member_state(
    repo: &MemoryWorkspaceRepository,
    a: MemberInstanceId,
    b: MemberInstanceId,
    own_instance: MemberInstanceId,
) {
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    let first = admission_change_for(a, &DeviceId::new("device-a"), SPACE);
    state
        .apply(WorkspaceConvergenceEvent::CommittedChange(first), 1)
        .unwrap();
    let first_digest = uc_core::membership::compute_change_digest(&state.changes[0]);
    let mut second = admission_change_for(b, &DeviceId::new("device-b"), SPACE);
    second.previous_epoch = 1;
    second.next_epoch = 2;
    second.previous_digest = first_digest;
    second.digest = uc_core::membership::compute_change_digest(&second);
    state
        .apply(WorkspaceConvergenceEvent::CommittedChange(second), 1)
        .unwrap();
    state
        .apply(
            WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance },
            1,
        )
        .unwrap();
    repo.save_state(&state).await.unwrap();
}

#[tokio::test]
async fn exchange_endpoint_accepts_a_remote_intent_and_forms_the_same_change() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    seeded_two_member_state(&harness.repository, a, b, a).await;

    let intent = signed_intent_for(SPACE, a, b);
    let reply = harness
        .owner
        .handle_exchange(
            &DeviceId::new("device-a"),
            RemovalExchangeMessage::Intent(Box::new(intent.clone())),
        )
        .await
        .unwrap();
    assert_eq!(reply, RemovalExchangeMessage::IntentAck(intent.intent_id));

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.removal_intents.len(), 1);
    assert_eq!(
        state.effective_members(),
        std::collections::BTreeSet::from([a])
    );
    assert_eq!(state.changes.len(), 3);
    assert!(!state.removed);
}

#[tokio::test]
async fn exchange_endpoint_rejects_a_non_member_source() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    seeded_two_member_state(&harness.repository, a, b, a).await;

    let intent = signed_intent_for(SPACE, a, b);
    let result = harness
        .owner
        .handle_exchange(
            &DeviceId::new("device-unknown"),
            RemovalExchangeMessage::Intent(Box::new(intent)),
        )
        .await;
    assert!(matches!(
        result,
        Err(uc_core::membership::RemovalExchangeError::Rejected)
    ));
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.removal_intent_records.is_empty());
}

#[tokio::test]
async fn late_submission_is_bounded_and_idempotent() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    seeded_two_member_state(&harness.repository, a, b, a).await;

    let intent = signed_intent_for(SPACE, a, b);
    let first = harness
        .owner
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent.clone())))
        .await
        .unwrap();
    assert!(matches!(first, RemovalLateAcceptance::Accepted { .. }));

    let second = harness
        .owner
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)))
        .await
        .unwrap();
    assert!(matches!(second, RemovalLateAcceptance::AlreadyKnown { .. }));

    let wrong = harness
        .owner
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(signed_intent_for(
            "other-space",
            a,
            b,
        ))))
        .await
        .unwrap();
    assert!(matches!(
        wrong,
        RemovalLateAcceptance::Rejected {
            reason: RemovalLateRejectionReason::InvalidSpaceLineage
        }
    ));
}

#[tokio::test]
async fn notice_marks_the_local_instance_removed() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    seeded_two_member_state(&harness.repository, a, b, b).await;

    let notice = RemovalNotice {
        space_lineage_fingerprint: RemovalNotice::space_lineage_fingerprint(SPACE),
        intent_id: uc_core::membership::RemovalIntentId::from_bytes([7; 32]),
        target_instance: b,
        target_device_id: DeviceId::new("device-b"),
        issuer_instance: a,
        signature: vec![1, 2, 3],
    };
    let acceptance = harness.owner.handle_notice(notice.clone()).await.unwrap();
    assert!(matches!(
        acceptance,
        RemovalNoticeAcceptance::Accepted { .. }
    ));

    let snapshot = harness.owner.query().await.unwrap();
    assert!(snapshot.removed);

    // Idempotent for the same intent.
    let again = harness.owner.handle_notice(notice).await.unwrap();
    assert!(matches!(
        again,
        RemovalNoticeAcceptance::AlreadyKnown { .. }
    ));
}

#[tokio::test]
async fn intent_targeting_the_local_instance_marks_removed() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    seeded_two_member_state(&harness.repository, a, b, b).await;

    let intent = signed_intent_for(SPACE, a, b);
    harness
        .owner
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)))
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.removed);
    assert_eq!(
        state.effective_members(),
        std::collections::BTreeSet::from([a])
    );
}

#[tokio::test]
async fn reconcile_propagates_intents_and_removal_notices() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let exchange = Arc::new(UnusedExchange::default());
    let repository = MemoryWorkspaceRepository::default();
    let owner = WorkspaceConvergence::new(WorkspaceConvergenceDeps {
        repository: Arc::new(repository.clone()),
        verification: Arc::new(AcceptingVerifier),
        recovery: Arc::new(FakeRecovery::new(
            DeviceId::new("device-a"),
            vec![
                (DeviceId::new("device-a"), a),
                (DeviceId::new("device-b"), b),
                (DeviceId::new("device-c"), c),
            ],
        )),
        member_signatures: Arc::new(FixedSigner),
        member_repo: Arc::new(uc_application_test_member_repo()),
        membership_identity: Arc::new(FixedMembershipIdentity {
            space: SpaceId::from_str(SPACE),
        }),
        announcement_material: Arc::new(FixedAnnouncementMaterial),
        security_updates: Arc::new(UnusedSecurityUpdates),
        clock: Arc::new(UnusedClock),
        device_identity: Arc::new(UnusedDeviceIdentity),
        exchange: Arc::clone(&exchange) as Arc<dyn uc_core::membership::RemovalExchangePort>,
        late_submission: Arc::new(UnusedLate),
        notice: Arc::new(UnusedNotice),
        notice_verification: Arc::new(RejectingNoticeVerification),
        recovery_transport: Arc::new(UnusedRecoveryTransport),
        trusted_peer_repo: Arc::new(TestTrustedPeerRepo),
        peer_addr_repo: Arc::new(TestPeerAddrRepo),
        own_device: DeviceId::new("device-a"),
    });
    seeded_two_member_state(&repository, a, b, a).await;
    let mut state = repository.load_state().await.unwrap().unwrap();
    let mut third = admission_change_for(c, &DeviceId::new("device-c"), SPACE);
    third.previous_epoch = 2;
    third.next_epoch = 3;
    third.previous_digest = uc_core::membership::compute_change_digest(&state.changes[1]);
    third.digest = uc_core::membership::compute_change_digest(&third);
    state
        .apply(WorkspaceConvergenceEvent::CommittedChange(third), 1)
        .unwrap();
    repository.save_state(&state).await.unwrap();
    owner
        .submit_removal(&DeviceId::new("device-c"))
        .await
        .unwrap();

    owner.reconcile().await.unwrap();

    let sent = exchange.sent.lock().unwrap();
    assert!(sent
        .iter()
        .any(|m| matches!(m, uc_core::membership::RemovalExchangeMessage::Intent(_))));
    assert!(sent.iter().any(|m| matches!(
        m,
        uc_core::membership::RemovalExchangeMessage::Confirmation(_)
    )));
    let state = repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.peer_intent_acks.len(), 1);
    assert_eq!(state.notified_removals.len(), 1);
}
