//! WorkspaceConvergence owner tests (ADR-016 flow semantics).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, CurrentMembershipIdentity,
    CurrentMembershipIdentityError, CurrentMembershipIdentityPort, MemberInstanceId,
    MemberRepositoryPort, MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
    RemovalCausalProof, RemovalCausalProofMember, RemovalIntentVerificationError,
    RemovalIntentVerificationPort, RemovalRecoveryError, RemovalRecoveryPort, RemovalViewMember,
    RemovalViewSnapshot, SignedRemovalIntent, WorkspaceChange, WorkspaceChangeKind,
    WorkspaceConvergenceEvent, WorkspaceConvergenceRepositoryError,
    WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState, WorkspacePhase,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort};

use crate::workspace_convergence::{
    WorkspaceConvergence, WorkspaceConvergenceDeps, WorkspaceConvergenceError,
};

const SPACE: &str = "test-workspace";

#[derive(Clone, Default)]
struct MemoryWorkspaceRepository {
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

    async fn prepare_key_package(&self) -> Result<Vec<u8>, RemovalRecoveryError> {
        Ok(b"kp".to_vec())
    }

    async fn prepare_forward_recovery(
        &self,
        _convergence_digest: &[u8; 32],
        _effective_members: &[MemberInstanceId],
        _key_packages: &[(MemberInstanceId, Vec<u8>)],
    ) -> Result<uc_core::membership::RemovalPreparedRecovery, RemovalRecoveryError> {
        Err(RemovalRecoveryError::Unavailable)
    }

    async fn install_prepared_forward_recovery(
        &self,
        _local_checkpoint: &[u8],
    ) -> Result<(), RemovalRecoveryError> {
        Err(RemovalRecoveryError::Unavailable)
    }

    async fn apply_forward_recovery(
        &self,
        _material: &uc_core::membership::RemovalRecoveryMaterial,
        _expected_convergence_digest: &[u8; 32],
        _expected_effective_members: &[MemberInstanceId],
    ) -> Result<(), RemovalRecoveryError> {
        Err(RemovalRecoveryError::Unavailable)
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
    let owner = WorkspaceConvergence::new(WorkspaceConvergenceDeps {
        repository: Arc::new(repository.clone()),
        verification: Arc::new(AcceptingVerifier),
        recovery: Arc::new(FakeRecovery::new(DeviceId::new(own_device), members)),
        member_signatures: Arc::new(FixedSigner),
        member_repo: Arc::new(uc_application_test_member_repo()),
        membership_identity: Arc::new(FixedMembershipIdentity {
            space: SpaceId::from_str(SPACE),
        }),
        security_updates: Arc::new(UnusedSecurityUpdates),
        clock: Arc::new(UnusedClock),
        device_identity: Arc::new(UnusedDeviceIdentity),
        own_device: DeviceId::new(own_device),
    });
    Harness { owner, repository }
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
