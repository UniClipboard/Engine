use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureVerifier, MembershipBranchRecoveryPackageV1,
    MembershipBranchTransitionV1, MembershipConflictChoice, MembershipConflictId,
    MembershipConflictPolicy, MembershipCredential, VersionedMembershipHistory,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::ClockPort;

use super::*;
use crate::space::membership::testing::{
    started_record, AcceptingVerifier, MemoryMembershipRecords, OwnerFixture,
};
use crate::space::membership::{
    MembershipBranchRecoverySession, MembershipConflictRecord, MembershipConflictStatus,
    MembershipOwner, MembershipRecord, SpaceMembershipRecord,
};

struct FixedClock;

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        100
    }
}

/// 成员记录与其负责人；测试直接改写记录时同步丢弃已发布状态。
struct Repository {
    records: Arc<MemoryMembershipRecords>,
    owner: Arc<MembershipOwner>,
}

impl Repository {
    fn space(&self) -> SpaceMembershipRecord {
        self.records.space()
    }

    fn commits(&self) -> usize {
        self.records.commit_count()
    }

    fn edit(&self, change: impl FnOnce(&mut SpaceMembershipRecord)) {
        let mut space = self.records.space();
        change(&mut space);
        self.records
            .replace(MembershipRecord::Space(Box::new(space)));
        self.owner.reload_for_test();
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
        Err(crate::space::membership::CurrentMemberSignatureError::invalid_state())
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

#[async_trait]
impl AdvanceMembershipBranchTransitionPort for TransitionPreparer {
    async fn advance_membership_branch_transition(
        &self,
        input: AdvanceMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, AdvanceMembershipBranchTransitionError> {
        let next_phase = match input.transition.phase() {
            uc_core::membership::MembershipBranchTransitionPhaseV1::Prepared => {
                uc_core::membership::MembershipBranchTransitionPhaseV1::SourceBackedUp
            }
            uc_core::membership::MembershipBranchTransitionPhaseV1::SourceBackedUp => {
                uc_core::membership::MembershipBranchTransitionPhaseV1::TargetVerified
            }
            uc_core::membership::MembershipBranchTransitionPhaseV1::TargetVerified => {
                uc_core::membership::MembershipBranchTransitionPhaseV1::TargetStaged
            }
            uc_core::membership::MembershipBranchTransitionPhaseV1::TargetStaged => {
                uc_core::membership::MembershipBranchTransitionPhaseV1::Promoted
            }
            uc_core::membership::MembershipBranchTransitionPhaseV1::Promoted => {
                uc_core::membership::MembershipBranchTransitionPhaseV1::RuntimeRestored
            }
            uc_core::membership::MembershipBranchTransitionPhaseV1::RuntimeRestored => {
                uc_core::membership::MembershipBranchTransitionPhaseV1::Completed
            }
            uc_core::membership::MembershipBranchTransitionPhaseV1::Completed => {
                return Err(AdvanceMembershipBranchTransitionError::Invalid {
                    source: anyhow::anyhow!("test transition is already complete"),
                });
            }
        };
        input.transition.advance(next_phase).ok_or_else(|| {
            AdvanceMembershipBranchTransitionError::Invalid {
                source: anyhow::anyhow!("invalid test transition advance"),
            }
        })
    }
}

struct Fixture {
    repository: Repository,
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
            device_id,
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
        history_bytes,
        vec![4],
        vec![5],
    )
    .unwrap()
    .with_authorization_signature(vec![6]);
    let MembershipRecord::Space(mut record) = started_record(history, device_id, member, 7) else {
        unreachable!("started record has a space");
    };
    record.branch_recovery.conflicts.insert(
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
    let owner_fixture = OwnerFixture::new(MembershipRecord::Space(record));
    let verifier: Arc<dyn HistoricalMembershipSignatureVerifier> = Arc::new(AcceptingVerifier);
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
        owner_fixture.owner.clone(),
        recovery.clone(),
        recipient.clone(),
        transition.clone(),
        transition.clone(),
        verifier,
        Arc::new(FixedClock),
    );
    Fixture {
        repository: Repository {
            records: owner_fixture.records.clone(),
            owner: owner_fixture.owner.clone(),
        },
        recovery,
        recipient,
        transition,
        use_case,
        conflict_id,
        nonce,
        transition_id,
    }
}

/// 让本机分支成为分叉中的本地分支，供恢复包签发方测试使用。
fn issuer_fixture(
    fixture: &Fixture,
    fail_first_commit: bool,
) -> (
    IssueMembershipBranchRecoveryUseCase,
    Arc<RecoveryMaterialSource>,
    DeviceId,
    uc_core::membership::MemberInstanceId,
    uc_core::membership::MembershipBranchId,
) {
    let space = fixture.repository.space();
    let local_device_id = space.ledger.local_device_id;
    let recipient_member = space.ledger.local_member;
    let target_branch_id = MembershipConflictPolicy::branch_id(&space.ledger.history).unwrap();
    let conflict_id = fixture.conflict_id;
    fixture.repository.edit(|space| {
        space
            .branch_recovery
            .conflicts
            .get_mut(&conflict_id)
            .unwrap()
            .local_branch_id = target_branch_id;
    });
    let material = Arc::new(RecoveryMaterialSource {
        calls: AtomicUsize::new(0),
        group_info_calls: AtomicUsize::new(0),
        commit_calls: AtomicUsize::new(0),
        fail_first_commit: AtomicBool::new(fail_first_commit),
    });
    let issuer = IssueMembershipBranchRecoveryUseCase::new(
        fixture.repository.owner.clone(),
        material.clone(),
        Arc::new(RecoverySigner),
        Arc::new(FixedClock),
    );
    (
        issuer,
        material,
        local_device_id,
        recipient_member,
        target_branch_id,
    )
}

#[tokio::test]
async fn valid_package_consumes_nonce_and_saves_prepared_transition_atomically() {
    let fixture = fixture();

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    let persisted = fixture.repository.space().branch_recovery;
    assert_eq!(fixture.repository.commits(), 3);
    assert_eq!(
        persisted.consumed_recovery_nonces[&fixture.nonce],
        fixture.conflict_id
    );
    assert_eq!(
        persisted.conflicts[&fixture.conflict_id].status,
        MembershipConflictStatus::Transitioning
    );
    assert!(persisted
        .branch_transitions
        .contains_key(&fixture.transition_id));
}

#[tokio::test]
async fn retry_after_commit_advances_without_fetching_or_preparing_again() {
    let fixture = fixture();
    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );

    assert_eq!(fixture.repository.commits(), 9);
    assert_eq!(fixture.recovery.group_info_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.recovery.submit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.recipient.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.transition.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nonce_consumed_by_another_conflict_never_creates_a_transition() {
    let fixture = fixture();
    let before = fixture.repository.space();
    let nonce = fixture.nonce;
    fixture.repository.edit(|space| {
        space
            .branch_recovery
            .consumed_recovery_nonces
            .insert(nonce, MembershipConflictId::from_bytes([0x61; 32]));
    });
    let expected = fixture.repository.space();

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::StableFailure
    );
    let persisted = fixture.repository.space();
    assert_eq!(fixture.repository.commits(), 2);
    assert_eq!(
        persisted.branch_recovery.consumed_recovery_nonces,
        expected.branch_recovery.consumed_recovery_nonces
    );
    assert!(persisted.branch_recovery.branch_transitions.is_empty());
    assert_eq!(before.ledger.revision, expected.ledger.revision);
}

#[tokio::test]
async fn retry_from_recipient_prepared_reuses_staged_state_without_group_info() {
    let fixture = fixture();
    let space = fixture.repository.space();
    let recipient_member = space.ledger.local_member;
    let target_branch_id = space.branch_recovery.conflicts[&fixture.conflict_id]
        .selected_branch_id
        .unwrap();
    let (transition_id, conflict_id) = (fixture.transition_id, fixture.conflict_id);
    fixture.repository.edit(|space| {
        space.branch_recovery.recovery_sessions.insert(
            transition_id,
            MembershipBranchRecoverySession::new_recipient_prepared(
                transition_id,
                conflict_id,
                target_branch_id,
                recipient_member,
                vec![0x61],
                vec![0x62],
            )
            .unwrap(),
        );
    });

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    assert_eq!(fixture.recovery.group_info_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.recipient.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.recovery.submit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.repository.commits(), 2);
}

#[tokio::test]
async fn issuer_authenticates_recipient_before_preparing_and_signing_material() {
    let fixture = fixture();
    let verifier: Arc<dyn HistoricalMembershipSignatureVerifier> = Arc::new(AcceptingVerifier);
    let (issuer, material, local_device_id, recipient_member, target_branch_id) =
        issuer_fixture(&fixture, false);
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
            source_device_id: local_device_id.clone(),
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
    let (issuer, material, local_device_id, recipient_member, target_branch_id) =
        issuer_fixture(&fixture, false);

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
    let persisted = fixture.repository.space().branch_recovery;
    let session = persisted.recovery_sessions.get(&transition_id).unwrap();
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 1);
    assert!(format!("{session:?}").contains("TargetCommitted"));
    assert_eq!(session.recipient_completion().map(|(_, value)| value), None);
    assert_eq!(package.conflict_id(), fixture.conflict_id);
    assert_eq!(
        persisted.conflicts[&fixture.conflict_id].status,
        MembershipConflictStatus::Completed
    );
}

#[tokio::test]
async fn target_recovery_resumes_from_prepared_after_commit_interruption() {
    let fixture = fixture();
    let (issuer, material, local_device_id, recipient_member, target_branch_id) =
        issuer_fixture(&fixture, true);
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
    let cached = fixture.repository.space().branch_recovery.recovery_sessions[&transition_id]
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
async fn prepared_transition_resumes_all_durable_phases_in_one_round() {
    let fixture = fixture();

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );
    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Completed
    );

    let persisted = fixture.repository.space().branch_recovery;
    assert_eq!(
        persisted
            .conflicts
            .get(&fixture.conflict_id)
            .unwrap()
            .status,
        MembershipConflictStatus::Completed
    );
    assert!(persisted.recovery_sessions.is_empty());
    assert_eq!(
        persisted
            .branch_transitions
            .get(&fixture.transition_id)
            .unwrap()
            .phase(),
        uc_core::membership::MembershipBranchTransitionPhaseV1::Completed
    );
}

#[tokio::test]
async fn target_recovery_retry_returns_the_cached_package_without_reapplying_commit() {
    let fixture = fixture();
    let (issuer, material, local_device_id, recipient_member, target_branch_id) =
        issuer_fixture(&fixture, false);
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
