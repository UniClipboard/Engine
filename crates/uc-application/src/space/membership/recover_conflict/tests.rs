use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipBranchRecoveryPackageV1, MembershipBranchTransitionV1, MembershipConflictChoice,
    MembershipConflictId, MembershipConflictPolicy, MembershipCredential,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::ClockPort;

use super::*;
use crate::connectivity::ConnectivityError;
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
    retired: Arc<AtomicBool>,
    retire_after_group_info: AtomicBool,
}

struct TestRecoveryScope(Arc<AtomicBool>);

impl super::ports::BranchRecoveryIssuerScope for TestRecoveryScope {
    fn check_frozen(&self) -> Result<(), ConnectivityError> {
        if self.0.load(Ordering::SeqCst) {
            Err(ConnectivityError::Denied)
        } else {
            Ok(())
        }
    }
}

fn issuer_scope() -> Arc<dyn super::ports::BranchRecoveryIssuerScope> {
    Arc::new(TestRecoveryScope(Arc::new(AtomicBool::new(false))))
}

#[async_trait]
impl BranchRecoveryOperationScopePort for TestRecoveryScope {
    fn check_frozen(&self) -> Result<(), MembershipBranchRecoveryChannelError> {
        if self.0.load(Ordering::SeqCst) {
            Err(MembershipBranchRecoveryChannelError::Unavailable {
                source: anyhow::anyhow!("test scope retired"),
            })
        } else {
            Ok(())
        }
    }

    async fn exchange(
        &self,
        _request: BranchRecoveryRequest,
    ) -> Result<BranchRecoveryResponse, MembershipBranchRecoveryChannelError> {
        unreachable!("test channel supplies the responses")
    }
}

/// 模拟 GroupInfo 返回前切换数据库，即使新库内容相同也不能延续旧操作。
struct SwitchingRecoverySource {
    records: Arc<MemoryMembershipRecords>,
    submit_calls: AtomicUsize,
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for SwitchingRecoverySource {
    fn freeze(
        &self,
        _peer: DeviceId,
    ) -> Result<Arc<dyn BranchRecoveryOperationScopePort>, MembershipBranchRecoveryChannelError>
    {
        Ok(Arc::new(TestRecoveryScope(Arc::new(AtomicBool::new(
            false,
        )))))
    }
    async fn request_membership_branch_group_info(
        &self,
        _request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        self.records.replace_database(self.records.record());
        Ok(vec![0x60])
    }

    async fn submit_membership_branch_external_commit(
        &self,
        _request: MembershipBranchRecoveryCommit,
    ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        Err(MembershipBranchRecoveryChannelError::Unavailable {
            source: anyhow::anyhow!("unexpected submission"),
        })
    }
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for RecoverySource {
    fn freeze(
        &self,
        _peer: DeviceId,
    ) -> Result<Arc<dyn BranchRecoveryOperationScopePort>, MembershipBranchRecoveryChannelError>
    {
        Ok(Arc::new(TestRecoveryScope(self.retired.clone())))
    }
    async fn request_membership_branch_group_info(
        &self,
        _request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        self.group_info_calls.fetch_add(1, Ordering::SeqCst);
        if self.retire_after_group_info.swap(false, Ordering::SeqCst) {
            self.retired.store(true, Ordering::SeqCst);
        }
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
    switch_after_prepare: AtomicBool,
    records: Arc<MemoryMembershipRecords>,
    clear_conflict_after_prepare: AtomicBool,
    clear_conflict_after_export: AtomicBool,
    owner: Arc<MembershipOwner>,
    block_commit: AtomicBool,
    block_prepare: AtomicBool,
    prepare_started: tokio::sync::Semaphore,
    prepare_resume: tokio::sync::Semaphore,
    commit_started: tokio::sync::Semaphore,
    commit_resume: tokio::sync::Semaphore,
    installed: Mutex<Option<Vec<u8>>>,
    install_count: AtomicUsize,
    fail_checkpoint_after_install: AtomicBool,
}

#[async_trait]
impl PrepareMembershipBranchRecoveryMaterialPort for RecoveryMaterialSource {
    async fn export_membership_branch_recovery_group_info(
        &self,
    ) -> Result<Vec<u8>, PrepareMembershipBranchRecoveryMaterialError> {
        self.group_info_calls.fetch_add(1, Ordering::SeqCst);
        if self
            .clear_conflict_after_export
            .swap(false, Ordering::SeqCst)
        {
            self.owner
                .commit(|draft| {
                    draft.branch_recovery_mut()?.conflicts.clear();
                    Ok(())
                })
                .await
                .unwrap();
        }
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
        if self.block_prepare.load(Ordering::SeqCst) {
            self.prepare_started.add_permits(1);
            self.prepare_resume.acquire().await.unwrap().forget();
        }
        if self.switch_after_prepare.swap(false, Ordering::SeqCst) {
            self.records.replace_database(self.records.record());
        }
        if self
            .clear_conflict_after_prepare
            .swap(false, Ordering::SeqCst)
        {
            self.owner
                .commit(|draft| {
                    draft.branch_recovery_mut()?.conflicts.clear();
                    Ok(())
                })
                .await
                .unwrap();
        }
        Ok(PreparedMembershipBranchRecoveryMaterial {
            target_staged_space_material: vec![0x70],
            sealed_mls_recovery_material: vec![0x71],
            encrypted_content_key_catalog: vec![0x72],
        })
    }

    async fn commit_membership_branch_recovery_material(
        &self,
        target_staged_space_material: Vec<u8>,
    ) -> Result<(), PrepareMembershipBranchRecoveryMaterialError> {
        self.commit_calls.fetch_add(1, Ordering::SeqCst);
        if self.block_commit.load(Ordering::SeqCst) {
            self.commit_started.add_permits(1);
            self.commit_resume.acquire().await.unwrap().forget();
        }
        if self.fail_first_commit.swap(false, Ordering::SeqCst) {
            return Err(PrepareMembershipBranchRecoveryMaterialError::Unavailable {
                source: anyhow::anyhow!("injected target commit interruption"),
            });
        }
        {
            let mut installed = self.installed.lock().unwrap();
            if let Some(current) = installed.as_ref() {
                assert_eq!(current, &target_staged_space_material);
            } else {
                *installed = Some(target_staged_space_material);
                self.install_count.fetch_add(1, Ordering::SeqCst);
            }
        }
        if self
            .fail_checkpoint_after_install
            .swap(false, Ordering::SeqCst)
        {
            self.records.fail_next_commits(1);
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
        retired: Arc::new(AtomicBool::new(false)),
        retire_after_group_info: AtomicBool::new(false),
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
        switch_after_prepare: AtomicBool::new(false),
        records: fixture.repository.records.clone(),
        clear_conflict_after_prepare: AtomicBool::new(false),
        clear_conflict_after_export: AtomicBool::new(false),
        owner: fixture.repository.owner.clone(),
        block_commit: AtomicBool::new(false),
        block_prepare: AtomicBool::new(false),
        prepare_started: tokio::sync::Semaphore::new(0),
        prepare_resume: tokio::sync::Semaphore::new(0),
        commit_started: tokio::sync::Semaphore::new(0),
        commit_resume: tokio::sync::Semaphore::new(0),
        installed: Mutex::new(None),
        install_count: AtomicUsize::new(0),
        fail_checkpoint_after_install: AtomicBool::new(false),
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
async fn retired_scope_stops_after_group_info_before_preparing_or_committing() {
    let fixture = fixture();
    fixture
        .recovery
        .retire_after_group_info
        .store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::Deferred
    );
    assert_eq!(fixture.recipient.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.recovery.submit_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.repository.commits(), 0);
}

#[tokio::test]
async fn database_replacement_after_group_info_does_not_continue_old_recovery() {
    let fixture = fixture();
    let source = Arc::new(SwitchingRecoverySource {
        records: fixture.repository.records.clone(),
        submit_calls: AtomicUsize::new(0),
    });
    let use_case = RecoverMembershipConflictUseCase::new(
        fixture.repository.owner.clone(),
        source.clone(),
        fixture.recipient.clone(),
        fixture.transition.clone(),
        fixture.transition.clone(),
        Arc::new(AcceptingVerifier),
        Arc::new(FixedClock),
    );

    assert_eq!(
        use_case.execute().await,
        RecoverMembershipConflictOutcome::Deferred
    );
    assert_eq!(fixture.recipient.calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.submit_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.repository.commits(), 0);
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
async fn issuer_rejects_retired_request_after_material_prepare() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material.block_prepare.store(true, Ordering::SeqCst);
    let retired = Arc::new(AtomicBool::new(false));
    let old_scope = Arc::new(TestRecoveryScope(retired.clone()));
    let conflict_id = fixture.conflict_id;
    let issuing = tokio::spawn(async move {
        issuer
            .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
                scope: old_scope,
                source_device_id: device,
                conflict_id,
                target_branch_id: branch,
                recipient_member: recipient,
                external_commit: vec![0x73],
            })
            .await
    });
    material.prepare_started.acquire().await.unwrap().forget();
    retired.store(true, Ordering::SeqCst);
    // 新范围仍可用，不能使原范围重新有效。
    let new_scope = issuer_scope();
    assert!(new_scope.check_frozen().is_ok());
    material.prepare_resume.add_permits(1);
    assert!(matches!(
        issuing.await.unwrap(),
        Err(IssueMembershipBranchRecoveryError::Unavailable { .. })
    ));
    assert_eq!(fixture.repository.commits(), 0);
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 0);
    assert!(fixture
        .repository
        .space()
        .branch_recovery
        .recovery_sessions
        .is_empty());
}

#[tokio::test]
async fn issuer_finishes_started_installation_after_scope_retirement() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material.block_commit.store(true, Ordering::SeqCst);
    let retired = Arc::new(AtomicBool::new(false));
    let scope = Arc::new(TestRecoveryScope(retired.clone()));
    let conflict_id = fixture.conflict_id;
    let transition_id = MembershipBranchTransitionV1::derive_id(conflict_id, branch);
    let issuing = tokio::spawn(async move {
        issuer
            .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
                scope,
                source_device_id: device,
                conflict_id,
                target_branch_id: branch,
                recipient_member: recipient,
                external_commit: vec![0x73],
            })
            .await
    });
    material.commit_started.acquire().await.unwrap().forget();
    retired.store(true, Ordering::SeqCst);
    material.commit_resume.add_permits(1);
    assert!(issuing.await.unwrap().is_ok());
    assert_eq!(material.install_count.load(Ordering::SeqCst), 1);
    assert!(
        fixture.repository.space().branch_recovery.recovery_sessions[&transition_id]
            .target_completion()
            .is_some()
    );
}

#[tokio::test]
async fn issuer_material_installation_serializes_conflict_change() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material.block_commit.store(true, Ordering::SeqCst);
    let conflict_id = fixture.conflict_id;
    let issuing = tokio::spawn(async move {
        issuer
            .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
                scope: issuer_scope(),
                source_device_id: device,
                conflict_id,
                target_branch_id: branch,
                recipient_member: recipient,
                external_commit: vec![0x73],
            })
            .await
    });
    material.commit_started.acquire().await.unwrap().forget();
    let owner = fixture.repository.owner.clone();
    let transition_id = MembershipBranchTransitionV1::derive_id(conflict_id, branch);
    let mut change = tokio::spawn(async move {
        owner
            .commit(|draft| {
                let record = draft.branch_recovery_mut()?;
                let completed = record
                    .recovery_sessions
                    .get(&transition_id)
                    .and_then(MembershipBranchRecoverySession::target_completion)
                    .is_some();
                record.conflicts.clear();
                Ok(completed)
            })
            .await
            .map(|result| result.output)
    });
    let early = tokio::time::timeout(std::time::Duration::from_millis(100), &mut change).await;
    material.commit_resume.add_permits(1);
    let completed_while_installing = early.is_ok();
    let observed_completed = match early {
        Ok(result) => result.unwrap().unwrap(),
        Err(_) => change.await.unwrap().unwrap(),
    };
    let _ = issuing.await.unwrap();
    assert!(
        !completed_while_installing,
        "安装期间成员事实不得越过提交边界"
    );
    assert!(observed_completed, "成员变更必须看到已完成的签发检查点");
}

#[tokio::test]
async fn issuer_does_not_return_group_info_after_conflict_changes() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material
        .clear_conflict_after_export
        .store(true, Ordering::SeqCst);
    let result = issuer
        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
            scope: issuer_scope(),
            source_device_id: device,
            conflict_id: fixture.conflict_id,
            target_branch_id: branch,
            recipient_member: recipient,
        })
        .await;
    assert!(matches!(
        result,
        Err(IssueMembershipBranchRecoveryError::Rejected { .. })
    ));
}

#[tokio::test]
async fn issuer_rechecks_authorization_if_conflict_changes_during_prepare() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material
        .clear_conflict_after_prepare
        .store(true, Ordering::SeqCst);
    let result = issuer
        .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
            scope: issuer_scope(),
            source_device_id: device,
            conflict_id: fixture.conflict_id,
            target_branch_id: branch,
            recipient_member: recipient,
            external_commit: vec![0x73],
        })
        .await;
    assert!(matches!(
        result,
        Err(IssueMembershipBranchRecoveryError::Rejected { .. })
    ));
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.repository.commits(), 1);
    assert!(fixture
        .repository
        .space()
        .branch_recovery
        .recovery_sessions
        .is_empty());
}

#[tokio::test]
async fn issuer_does_not_commit_material_after_database_replacement_during_prepare() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material.switch_after_prepare.store(true, Ordering::SeqCst);
    let result = issuer
        .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
            scope: issuer_scope(),
            source_device_id: device,
            conflict_id: fixture.conflict_id,
            target_branch_id: branch,
            recipient_member: recipient,
            external_commit: vec![0x73],
        })
        .await;
    assert!(matches!(
        result,
        Err(IssueMembershipBranchRecoveryError::Unavailable { .. })
    ));
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.repository.commits(), 0);
}

#[tokio::test]
async fn issuer_authenticates_recipient_before_preparing_and_signing_material() {
    let fixture = fixture();
    let verifier: Arc<dyn HistoricalMembershipSignatureVerifier> = Arc::new(AcceptingVerifier);
    let (issuer, material, local_device_id, recipient_member, target_branch_id) =
        issuer_fixture(&fixture, false);
    let request = IssueMembershipBranchRecoveryInput {
        scope: issuer_scope(),
        source_device_id: DeviceId::new("wrong-device"),
        conflict_id: fixture.conflict_id,
        target_branch_id,
        recipient_member,
        external_commit: vec![0x73],
    };

    let begin_rejected = issuer
        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
            scope: issuer_scope(),
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

    let invalid_recipient = MemberInstanceId::from_bytes([0x99; 32]);
    let invalid_begin = issuer
        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
            scope: issuer_scope(),
            source_device_id: local_device_id.clone(),
            conflict_id: fixture.conflict_id,
            target_branch_id,
            recipient_member: invalid_recipient,
        })
        .await;
    assert!(matches!(
        invalid_begin,
        Err(IssueMembershipBranchRecoveryError::Rejected { .. })
    ));
    assert_eq!(material.group_info_calls.load(Ordering::SeqCst), 0);
    let invalid_issue = issuer
        .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
            scope: issuer_scope(),
            source_device_id: local_device_id.clone(),
            recipient_member: invalid_recipient,
            ..request.clone()
        })
        .await;
    assert!(matches!(
        invalid_issue,
        Err(IssueMembershipBranchRecoveryError::Rejected { .. })
    ));
    assert_eq!(material.calls.load(Ordering::SeqCst), 0);

    let group_info = issuer
        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
            scope: issuer_scope(),
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
            scope: issuer_scope(),
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
            scope: issuer_scope(),
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
async fn restart_after_material_install_and_checkpoint_failure_reuses_same_commit() {
    let fixture = fixture();
    let (issuer, material, device, recipient, branch) = issuer_fixture(&fixture, false);
    material
        .fail_checkpoint_after_install
        .store(true, Ordering::SeqCst);
    let request = IssueMembershipBranchRecoveryInput {
        scope: issuer_scope(),
        source_device_id: device,
        conflict_id: fixture.conflict_id,
        target_branch_id: branch,
        recipient_member: recipient,
        external_commit: vec![0x73],
    };
    assert!(matches!(
        issuer
            .issue_membership_branch_recovery(request.clone())
            .await,
        Err(IssueMembershipBranchRecoveryError::Unavailable { .. })
    ));
    let transition_id = MembershipBranchTransitionV1::derive_id(fixture.conflict_id, branch);
    let cached = fixture.repository.space().branch_recovery.recovery_sessions[&transition_id]
        .target_preparation()
        .map(|(_, _, package)| package.clone())
        .unwrap();
    assert_eq!(material.install_count.load(Ordering::SeqCst), 1);
    drop(issuer);

    // 使用持久记录构造新的成员负责人，模拟进程重启而不是复用内存发布状态。
    let restarted = OwnerFixture::new(fixture.repository.records.record());
    let issuer = IssueMembershipBranchRecoveryUseCase::new(
        restarted.owner.clone(),
        material.clone(),
        Arc::new(RecoverySigner),
        Arc::new(FixedClock),
    );
    let mut conflicting = request.clone();
    conflicting.external_commit = vec![0x74];
    assert!(matches!(
        issuer.issue_membership_branch_recovery(conflicting).await,
        Err(IssueMembershipBranchRecoveryError::Rejected { .. })
    ));
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 1);
    let recovered = issuer
        .issue_membership_branch_recovery(request)
        .await
        .unwrap();
    assert_eq!(recovered, cached);
    assert_eq!(material.calls.load(Ordering::SeqCst), 1);
    assert_eq!(material.commit_calls.load(Ordering::SeqCst), 2);
    assert_eq!(material.install_count.load(Ordering::SeqCst), 1);
    assert!(
        restarted.records.space().branch_recovery.recovery_sessions[&transition_id]
            .target_completion()
            .is_some()
    );
}

#[tokio::test]
async fn target_recovery_resumes_from_prepared_after_commit_interruption() {
    let fixture = fixture();
    let (issuer, material, local_device_id, recipient_member, target_branch_id) =
        issuer_fixture(&fixture, true);
    let request = IssueMembershipBranchRecoveryInput {
        scope: issuer_scope(),
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
        scope: issuer_scope(),
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

#[tokio::test]
async fn a_stable_failure_is_recorded_once_with_its_stage_and_no_identifiers() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
    let fixture = fixture();
    let nonce = fixture.nonce;
    fixture.repository.edit(|space| {
        space
            .branch_recovery
            .consumed_recovery_nonces
            .insert(nonce, MembershipConflictId::from_bytes([0x61; 32]));
    });

    assert_eq!(
        fixture.use_case.execute().await,
        RecoverMembershipConflictOutcome::StableFailure
    );

    assert_eq!(
        logs.count("membership conflict recovery stopped with a stable failure"),
        1
    );
    assert!(logs.output().contains("stage=\"ledger\""));
    assert!(logs.output().contains("error_kind=\"conflict\""));
    assert!(!logs.output().contains("0x61"));
}
