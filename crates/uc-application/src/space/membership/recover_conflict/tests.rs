use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    MembershipBranchRecoverySession, MembershipConflictRecord, MembershipConflictStatus,
    MembershipLedger, MembershipLedgerError, MembershipLedgerMutation,
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
    group_info_calls: AtomicUsize,
    submit_calls: AtomicUsize,
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for RecoverySource {
    async fn request_membership_branch_group_info(
        &self,
        _request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        self.group_info_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0x60])
    }

    async fn submit_membership_branch_external_commit(
        &self,
        _request: MembershipBranchRecoveryCommit,
    ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.package.clone())
    }
}

struct RecipientPreparer {
    calls: AtomicUsize,
}

#[async_trait]
impl PrepareMembershipBranchRecoveryRecipientPort for RecipientPreparer {
    async fn prepare_membership_branch_recovery_recipient(
        &self,
        _group_info: Vec<u8>,
    ) -> Result<
        PreparedMembershipBranchRecoveryRecipient,
        PrepareMembershipBranchRecoveryRecipientError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(PreparedMembershipBranchRecoveryRecipient {
            external_commit: vec![0x61],
            staged_mls_state: vec![0x62],
        })
    }
}

struct TransitionPreparer {
    calls: AtomicUsize,
}

struct RecoveryMaterialSource {
    calls: AtomicUsize,
    group_info_calls: AtomicUsize,
    commit_calls: AtomicUsize,
    fail_first_commit: AtomicBool,
}

#[async_trait]
impl PrepareMembershipBranchRecoveryMaterialPort for RecoveryMaterialSource {
    async fn export_membership_branch_recovery_group_info(
        &self,
    ) -> Result<Vec<u8>, PrepareMembershipBranchRecoveryMaterialError> {
        self.group_info_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0x70])
    }

    async fn prepare_membership_branch_recovery_material(
        &self,
        _input: PrepareMembershipBranchRecoveryMaterialInput,
    ) -> Result<
        PreparedMembershipBranchRecoveryMaterial,
        PrepareMembershipBranchRecoveryMaterialError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(PreparedMembershipBranchRecoveryMaterial {
            target_staged_space_material: vec![0x70],
            sealed_mls_recovery_material: vec![0x71],
            encrypted_content_key_catalog: vec![0x72],
        })
    }

    async fn commit_membership_branch_recovery_material(
        &self,
        _target_staged_space_material: Vec<u8>,
    ) -> Result<(), PrepareMembershipBranchRecoveryMaterialError> {
        self.commit_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first_commit.swap(false, Ordering::SeqCst) {
            return Err(PrepareMembershipBranchRecoveryMaterialError::Unavailable {
                source: anyhow::anyhow!("injected target commit interruption"),
            });
        }
        Ok(())
    }
}

struct RecoverySigner;

#[async_trait]
impl crate::space::membership::CurrentMemberSignaturePort for RecoverySigner {
    async fn current_member_epoch(
        &self,
    ) -> Result<u64, crate::space::membership::CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn current_member_instance(
        &self,
        _device_id: &DeviceId,
    ) -> Result<
        uc_core::membership::MemberInstanceId,
        crate::space::membership::CurrentMemberSignatureError,
    > {
        Err(crate::space::membership::CurrentMemberSignatureError::InvalidState)
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, crate::space::membership::CurrentMemberSignatureError> {
        Ok(vec![0x73])
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, crate::space::membership::CurrentMemberSignatureError> {
        Ok(true)
    }
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
    recipient: Arc<RecipientPreparer>,
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
        group_info_calls: AtomicUsize::new(0),
        submit_calls: AtomicUsize::new(0),
    });
    let recipient = Arc::new(RecipientPreparer {
        calls: AtomicUsize::new(0),
    });
    let transition = Arc::new(TransitionPreparer {
        calls: AtomicUsize::new(0),
    });
    let use_case = RecoverMembershipConflictUseCase::new(
        ledger,
        recovery.clone(),
        recipient.clone(),
        transition.clone(),
        verifier,
        Arc::new(FixedClock),
    );
    Fixture {
        repository,
        recovery,
        recipient,
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
    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 3);
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

    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.recovery.group_info_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.recovery.submit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.recipient.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.transition.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nonce_consumed_by_another_conflict_never_creates_a_transition() {
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
    let persisted = fixture.repository.load().await.unwrap();
    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 2);
    assert_eq!(
        persisted.consumed_membership_recovery_nonces,
        expected.consumed_membership_recovery_nonces
    );
    assert!(persisted.membership_branch_transitions.is_empty());
    assert_eq!(before.revision, expected.revision);
}

#[tokio::test]
async fn retry_from_recipient_prepared_reuses_staged_state_without_group_info() {
    let fixture = fixture();
    let recipient_member = fixture
        .repository
        .record
        .lock()
        .unwrap()
        .local_member_instance
        .unwrap();
    let target_branch_id = fixture
        .repository
        .record
        .lock()
        .unwrap()
        .membership_conflicts[&fixture.conflict_id]
        .selected_branch_id
        .unwrap();
    fixture
        .repository
        .record
        .lock()
        .unwrap()
        .membership_branch_recovery_sessions
        .insert(
            fixture.transition_id,
            MembershipBranchRecoverySession::new_recipient_prepared(
                fixture.transition_id,
                fixture.conflict_id,
                target_branch_id,
                recipient_member,
                vec![0x61],
                vec![0x62],
            )
            .unwrap(),
        );

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    assert_eq!(fixture.recovery.group_info_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.recipient.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.recovery.submit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.repository.commits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn issuer_authenticates_recipient_before_preparing_and_signing_material() {
    let fixture = fixture();
    let (local_device_id, recipient_member, target_branch_id) = {
        let mut record = fixture.repository.record.lock().unwrap();
        let local_device_id = record.local_device_id.clone().unwrap();
        let recipient_member = record.local_member_instance.unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            record.membership_history.as_deref().unwrap(),
            &AcceptingVerifier,
        )
        .unwrap();
        let target_branch_id = MembershipConflictPolicy::branch_id(&history).unwrap();
        record
            .membership_conflicts
            .get_mut(&fixture.conflict_id)
            .unwrap()
            .local_branch_id = target_branch_id;
        (local_device_id, recipient_member, target_branch_id)
    };
    let verifier: Arc<dyn HistoricalMembershipSignatureVerifier> = Arc::new(AcceptingVerifier);
    let ledger = Arc::new(MembershipLedger::new(
        fixture.repository.clone(),
        fixture.repository.clone(),
        verifier.clone(),
    ));
    let material = Arc::new(RecoveryMaterialSource {
        calls: AtomicUsize::new(0),
        group_info_calls: AtomicUsize::new(0),
        commit_calls: AtomicUsize::new(0),
        fail_first_commit: AtomicBool::new(false),
    });
    let issuer = IssueMembershipBranchRecoveryUseCase::new(
        ledger,
        material.clone(),
        Arc::new(RecoverySigner),
        Arc::new(FixedClock),
    );
    let request = IssueMembershipBranchRecoveryInput {
        source_device_id: DeviceId::new("wrong-device"),
        conflict_id: fixture.conflict_id,
        target_branch_id,
        recipient_member,
        external_commit: vec![0x73],
    };

    let begin_rejected = issuer
        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
            source_device_id: DeviceId::new("wrong-device"),
            conflict_id: fixture.conflict_id,
            target_branch_id,
            recipient_member,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        begin_rejected,
        IssueMembershipBranchRecoveryError::Rejected { .. }
    ));
    assert_eq!(material.group_info_calls.load(Ordering::SeqCst), 0);

    let group_info = issuer
        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
            source_device_id: local_device_id.clone(),
            conflict_id: fixture.conflict_id,
            target_branch_id,
            recipient_member,
        })
        .await
        .unwrap();
    assert_eq!(group_info, vec![0x70]);
    assert_eq!(material.group_info_calls.load(Ordering::SeqCst), 1);

    let rejected = issuer
        .issue_membership_branch_recovery(request.clone())
        .await
        .unwrap_err();
    assert!(matches!(
        rejected,
        IssueMembershipBranchRecoveryError::Rejected { .. }
    ));
    assert_eq!(material.calls.load(Ordering::SeqCst), 0);

    let package = issuer
        .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
            source_device_id: local_device_id,
            ..request
        })
        .await
        .unwrap();
    package
        .validate(
            fixture.conflict_id,
            target_branch_id,
            recipient_member,
            100,
            verifier.as_ref(),
        )
        .unwrap();
    assert_eq!(material.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn target_recovery_commits_only_after_caching_an_idempotent_package() {
    let fixture = fixture();
    let (local_device_id, recipient_member, target_branch_id) = {
        let mut record = fixture.repository.record.lock().unwrap();
        let local_device_id = record.local_device_id.clone().unwrap();
        let recipient_member = record.local_member_instance.unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            record.membership_history.as_deref().unwrap(),
            &AcceptingVerifier,
        )
        .unwrap();
        let target_branch_id = MembershipConflictPolicy::branch_id(&history).unwrap();
        record
            .membership_conflicts
            .get_mut(&fixture.conflict_id)
            .unwrap()
            .local_branch_id = target_branch_id;
        (local_device_id, recipient_member, target_branch_id)
    };
    let ledger = Arc::new(MembershipLedger::new(
        fixture.repository.clone(),
        fixture.repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let material = Arc::new(RecoveryMaterialSource {
        calls: AtomicUsize::new(0),
        group_info_calls: AtomicUsize::new(0),
        commit_calls: AtomicUsize::new(0),
        fail_first_commit: AtomicBool::new(false),
    });
    let issuer = IssueMembershipBranchRecoveryUseCase::new(
        ledger,
        material.clone(),
        Arc::new(RecoverySigner),
        Arc::new(FixedClock),
    );

    let package = issuer
        .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
            source_device_id: local_device_id,
            conflict_id: fixture.conflict_id,
            target_branch_id,
            recipient_member,
            external_commit: vec![0x73],
        })
        .await
        .unwrap();

    let transition_id =
        MembershipBranchTransitionV1::derive_id(fixture.conflict_id, target_branch_id);
    let persisted = fixture.repository.load().await.unwrap();
    let session = persisted
        .membership_branch_recovery_sessions
        .get(&transition_id)
        .unwrap();
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 1);
    assert!(format!("{session:?}").contains("TargetCommitted"));
    assert_eq!(session.recipient_completion().map(|(_, value)| value), None);
    assert_eq!(package.conflict_id(), fixture.conflict_id);
}

#[tokio::test]
async fn target_recovery_resumes_from_prepared_after_commit_interruption() {
    let fixture = fixture();
    let (local_device_id, recipient_member, target_branch_id) = {
        let mut record = fixture.repository.record.lock().unwrap();
        let local_device_id = record.local_device_id.clone().unwrap();
        let recipient_member = record.local_member_instance.unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            record.membership_history.as_deref().unwrap(),
            &AcceptingVerifier,
        )
        .unwrap();
        let target_branch_id = MembershipConflictPolicy::branch_id(&history).unwrap();
        record
            .membership_conflicts
            .get_mut(&fixture.conflict_id)
            .unwrap()
            .local_branch_id = target_branch_id;
        (local_device_id, recipient_member, target_branch_id)
    };
    let ledger = Arc::new(MembershipLedger::new(
        fixture.repository.clone(),
        fixture.repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let material = Arc::new(RecoveryMaterialSource {
        calls: AtomicUsize::new(0),
        group_info_calls: AtomicUsize::new(0),
        commit_calls: AtomicUsize::new(0),
        fail_first_commit: AtomicBool::new(true),
    });
    let issuer = IssueMembershipBranchRecoveryUseCase::new(
        ledger,
        material.clone(),
        Arc::new(RecoverySigner),
        Arc::new(FixedClock),
    );
    let request = IssueMembershipBranchRecoveryInput {
        source_device_id: local_device_id,
        conflict_id: fixture.conflict_id,
        target_branch_id,
        recipient_member,
        external_commit: vec![0x73],
    };

    assert!(matches!(
        issuer
            .issue_membership_branch_recovery(request.clone())
            .await,
        Err(IssueMembershipBranchRecoveryError::Unavailable { .. })
    ));
    let transition_id =
        MembershipBranchTransitionV1::derive_id(fixture.conflict_id, target_branch_id);
    let cached = fixture
        .repository
        .load()
        .await
        .unwrap()
        .membership_branch_recovery_sessions[&transition_id]
        .target_preparation()
        .map(|(_, _, package)| package.clone())
        .unwrap();

    let resumed = issuer
        .issue_membership_branch_recovery(request)
        .await
        .unwrap();

    assert_eq!(resumed, cached);
    assert_eq!(material.calls.load(Ordering::SeqCst), 1);
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn target_recovery_retry_returns_the_cached_package_without_reapplying_commit() {
    let fixture = fixture();
    let (local_device_id, recipient_member, target_branch_id) = {
        let mut record = fixture.repository.record.lock().unwrap();
        let local_device_id = record.local_device_id.clone().unwrap();
        let recipient_member = record.local_member_instance.unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            record.membership_history.as_deref().unwrap(),
            &AcceptingVerifier,
        )
        .unwrap();
        let target_branch_id = MembershipConflictPolicy::branch_id(&history).unwrap();
        record
            .membership_conflicts
            .get_mut(&fixture.conflict_id)
            .unwrap()
            .local_branch_id = target_branch_id;
        (local_device_id, recipient_member, target_branch_id)
    };
    let ledger = Arc::new(MembershipLedger::new(
        fixture.repository.clone(),
        fixture.repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let material = Arc::new(RecoveryMaterialSource {
        calls: AtomicUsize::new(0),
        group_info_calls: AtomicUsize::new(0),
        commit_calls: AtomicUsize::new(0),
        fail_first_commit: AtomicBool::new(false),
    });
    let issuer = IssueMembershipBranchRecoveryUseCase::new(
        ledger,
        material.clone(),
        Arc::new(RecoverySigner),
        Arc::new(FixedClock),
    );
    let request = IssueMembershipBranchRecoveryInput {
        source_device_id: local_device_id,
        conflict_id: fixture.conflict_id,
        target_branch_id,
        recipient_member,
        external_commit: vec![0x73],
    };

    let first = issuer
        .issue_membership_branch_recovery(request.clone())
        .await
        .unwrap();
    let retried = issuer
        .issue_membership_branch_recovery(request)
        .await
        .unwrap();

    assert_eq!(first, retried);
    assert_eq!(material.calls.load(Ordering::SeqCst), 1);
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 1);
}
