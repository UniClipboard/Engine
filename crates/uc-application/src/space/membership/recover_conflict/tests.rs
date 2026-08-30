use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipBranchRecoveryPackageV1,
    MembershipBranchTransitionV1, MembershipConflictChoice, MembershipConflictId,
    MembershipConflictPolicy, MembershipCredential, VersionedMembershipHistory,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::ClockPort;

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipConflictRecord, MembershipConflictStatus, MembershipLedger, MembershipLedgerError,
    MembershipLedgerMutation,
};

struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
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

struct FixedClock;

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        100
    }
}

struct MemoryLedger {
    record: Mutex<LoadedMembershipLedger>,
    commits: AtomicUsize,
}

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.record
            .lock()
            .map(|record| record.clone())
            .map_err(|_| MembershipLedgerError::Unavailable)
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedger {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut current = self
            .record
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)?;
        let digest = current
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if current.revision != mutation.expected_revision
            || digest != mutation.expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        *current = mutation.replacement;
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(current.clone())
    }
}

struct RecoverySource {
    package: MembershipBranchRecoveryPackageV1,
    calls: AtomicUsize,
}

#[async_trait]
impl FetchMembershipBranchRecoveryPort for RecoverySource {
    async fn fetch_membership_branch_recovery(
        &self,
        _input: FetchMembershipBranchRecoveryInput,
    ) -> Result<MembershipBranchRecoveryPackageV1, FetchMembershipBranchRecoveryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.package.clone())
    }
}

struct TransitionPreparer {
    calls: AtomicUsize,
}

#[async_trait]
impl PrepareMembershipBranchTransitionPort for TransitionPreparer {
    async fn prepare_membership_branch_transition(
        &self,
        input: PrepareMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, PrepareMembershipBranchTransitionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        MembershipBranchTransitionV1::new(
            input.transition_id,
            input.conflict_id,
            input.target_branch_id,
            [1; 16],
            [2; 16],
        )
        .ok_or_else(|| PrepareMembershipBranchTransitionError::Invalid {
            source: anyhow::anyhow!("invalid test transition"),
        })
    }
}

struct Fixture {
    repository: Arc<MemoryLedger>,
    recovery: Arc<RecoverySource>,
    transition: Arc<TransitionPreparer>,
    use_case: RecoverMembershipConflictUseCase,
    conflict_id: MembershipConflictId,
    nonce: [u8; 32],
    transition_id: [u8; 32],
}

fn fixture() -> Fixture {
    let device_id = DeviceId::new("local");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
    let member = credential.member_instance_id(&device_id);
    let history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        AdmissionChangeFacts {
            member_instance: member,
            device_id: device_id.clone(),
            device_name: "Local".to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        credential,
    )
    .unwrap();
    let history_bytes = history.encode_persisted_v2().unwrap();
    let target_branch_id = MembershipConflictPolicy::branch_id(&history).unwrap();
    let conflict_id = MembershipConflictId::from_bytes([0x51; 32]);
    let transition_id = [0x52; 32];
    let nonce = [0x53; 32];
    let package = MembershipBranchRecoveryPackageV1::new_unsigned(
        conflict_id,
        target_branch_id,
        member,
        member,
        1_000,
        nonce,
        history_bytes.clone(),
        vec![4],
        vec![5],
    )
    .unwrap()
    .with_authorization_signature(vec![6]);
    let mut record = LoadedMembershipLedger::no_current_space();
    record.revision = 7;
    record.lineage_id = Some("space-a".to_owned());
    record.membership_history = Some(history_bytes);
    record.local_device_id = Some(device_id);
    record.local_member_instance = Some(member);
    record.local_join_active = true;
    record.membership_conflicts.insert(
        conflict_id,
        MembershipConflictRecord {
            conflict_id,
            local_branch_id: uc_core::membership::MembershipBranchId::from_bytes([0x54; 32]),
            remote_branch_id: target_branch_id,
            local_choice: MembershipConflictChoice::ActiveMemberRecovery,
            remote_choice: MembershipConflictChoice::ActiveMemberRecovery,
            evidence_peer_device_ids: BTreeSet::from([DeviceId::new("peer")]),
            detected_at_revision: 7,
            status: MembershipConflictStatus::Selected,
            selected_branch_id: Some(target_branch_id),
            transition_id: Some(transition_id),
        },
    );
    let repository = Arc::new(MemoryLedger {
        record: Mutex::new(record),
        commits: AtomicUsize::new(0),
    });
    let verifier: Arc<dyn HistoricalMembershipSignatureVerifier> = Arc::new(AcceptingVerifier);
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::clone(&verifier),
    ));
    let recovery = Arc::new(RecoverySource {
        package,
        calls: AtomicUsize::new(0),
    });
    let transition = Arc::new(TransitionPreparer {
        calls: AtomicUsize::new(0),
    });
    let use_case = RecoverMembershipConflictUseCase::new(
        ledger,
        recovery.clone(),
        transition.clone(),
        verifier,
        Arc::new(FixedClock),
    );
    Fixture {
        repository,
        recovery,
        transition,
        use_case,
        conflict_id,
        nonce,
        transition_id,
    }
}

#[tokio::test]
async fn valid_package_consumes_nonce_and_saves_prepared_transition_atomically() {
    let fixture = fixture();

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    let persisted = fixture.repository.load().await.unwrap();
    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(
        persisted.consumed_membership_recovery_nonces[&fixture.nonce],
        fixture.conflict_id
    );
    assert_eq!(
        persisted.membership_conflicts[&fixture.conflict_id].status,
        MembershipConflictStatus::Transitioning
    );
    assert!(persisted
        .membership_branch_transitions
        .contains_key(&fixture.transition_id));
}

#[tokio::test]
async fn retry_after_commit_does_not_fetch_or_prepare_again() {
    let fixture = fixture();
    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );

    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.recovery.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.transition.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nonce_consumed_by_another_conflict_has_zero_ledger_side_effects() {
    let fixture = fixture();
    let before = fixture.repository.load().await.unwrap();
    fixture
        .repository
        .record
        .lock()
        .unwrap()
        .consumed_membership_recovery_nonces
        .insert(fixture.nonce, MembershipConflictId::from_bytes([0x61; 32]));
    let expected = fixture.repository.load().await.unwrap();

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::StableFailure
    );
    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.repository.load().await.unwrap(), expected);
    assert_eq!(before.revision, expected.revision);
}
