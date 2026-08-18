use super::super::tests::*;

async fn seed_superseded_and_current_join(
    repository: &Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
) -> (
    uc_core::membership::AdmissionAttemptId,
    uc_core::membership::AdmissionAttemptId,
) {
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionAttemptV1, AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1,
        JoinerAdmissionStageV1, LocalJoinStartMutationV1, MemberInstanceId,
    };

    fn initiated(
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        ordinal: u64,
    ) -> AdmissionAttemptV1 {
        let mut attempt =
            AdmissionAttemptV1::new_joiner(attempt_id, join_id, JoinerAdmissionStageV1::Initiated);
        attempt.local_join_ordinal = Some(ordinal);
        attempt.joiner_pending_security_state = Some(vec![1]);
        attempt.candidate_key_package = Some(b"joiner-key-package".to_vec());
        attempt.joiner_member_instance = Some(MemberInstanceId::from_bytes([2; 32]));
        attempt.resume_public_key = Some(vec![3; 32]);
        attempt.resume_private_key = Some(vec![4; 32]);
        attempt.outboxes.push(AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::JoinRequest,
            recipient: b"sponsor".to_vec(),
            message_id: [5; 32],
            predecessor_message_id: None,
            payload: b"join-request".to_vec(),
            superseded: false,
        });
        attempt
    }

    let previous_id = AdmissionAttemptId::from_bytes([0xd1; 32]);
    let previous = initiated(previous_id, [0xd2; 16], 0);
    repository
        .commit_local_join_start(LocalJoinStartMutationV1::Create {
            replacement: previous.clone(),
        })
        .await
        .unwrap();
    let cleanup = AdmissionOutboxMessageV1 {
        purpose: AdmissionOutboxPurposeV1::CancelRequested,
        recipient: b"sponsor".to_vec(),
        message_id: [0xd3; 32],
        predecessor_message_id: Some([5; 32]),
        payload: b"cancel_requested".to_vec(),
        superseded: false,
    };
    let mut previous_terminal = previous.superseded_by_new_join(cleanup).unwrap();
    previous_terminal.record_version = 1;
    let current_id = AdmissionAttemptId::from_bytes([0xd4; 32]);
    let current = initiated(current_id, [0xd5; 16], 1);
    repository
        .commit_local_join_start(LocalJoinStartMutationV1::Supersede {
            expected_previous_attempt_id: previous_id,
            expected_previous_record_version: 0,
            previous_terminal,
            replacement: current,
        })
        .await
        .unwrap();
    (previous_id, current_id)
}

#[tokio::test]
async fn superseded_late_candidate_cannot_replace_current_join() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xd6; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let (candidate, history, event, commitment, _) =
        durable_candidate_verification_fixture(previous_id);
    let candidate_message = uc_core::membership::AdmissionOutboxMessageV1 {
        purpose: uc_core::membership::AdmissionOutboxPurposeV1::Candidate,
        recipient: b"joiner".to_vec(),
        message_id: [0xd7; 32],
        predecessor_message_id: Some([5; 32]),
        payload: b"candidate".to_vec(),
        superseded: false,
    };

    let cleanup = owner
        .joiner_verify_and_prepare(
            previous_id,
            &candidate_message,
            candidate,
            history,
            &event,
            &commitment,
            b"target-access",
            b"prepared-proof",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();

    assert_eq!(
        cleanup.purpose,
        uc_core::membership::AdmissionOutboxPurposeV1::CancelRequested
    );
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn superseded_late_candidate_is_recorded_through_the_protocol_entry() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xc0; 16]);
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let mut deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "device-1",
        Vec::new(),
    );
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceConvergence::new(deps);
    let candidate = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Candidate,
        b"device-1",
        Some([5; 32]),
        b"late-candidate",
    );
    let frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *previous_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Candidate,
        message_id: candidate.message_id,
        predecessor_message_id: candidate.predecessor_message_id,
        payload: candidate.payload,
    };

    assert!(matches!(
        owner
            .prepare_joiner_candidate(
                &frame,
                &FailingPreparation,
                &FailingTargetAccess,
                &uc_core::crypto::domain::Passphrase::new("passphrase"),
            )
            .await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
    let previous = repository.load(previous_id).await.unwrap().unwrap();
    assert!(previous
        .inbox_dedup
        .iter()
        .any(|record| record.message_id == frame.message_id));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn superseded_late_commit_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xd8; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let (_, _, _, _, receipt) = durable_candidate_verification_fixture(previous_id);
    let commit = uc_core::membership::AdmissionOutboxMessageV1 {
        purpose: uc_core::membership::AdmissionOutboxPurposeV1::Commit,
        recipient: b"joiner".to_vec(),
        message_id: [0xd9; 32],
        predecessor_message_id: Some([0xda; 32]),
        payload: b"commit".to_vec(),
        superseded: false,
    };

    assert!(matches!(
        owner
            .joiner_apply(previous_id, &commit, &receipt, b"sponsor", b"applied")
            .await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
    assert!(!repository
        .load(previous_id)
        .await
        .unwrap()
        .unwrap()
        .inbox_dedup
        .is_empty());
}

#[tokio::test]
async fn superseded_late_complete_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xde; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let complete = uc_core::membership::AdmissionOutboxMessageV1 {
        purpose: uc_core::membership::AdmissionOutboxPurposeV1::Complete,
        recipient: b"joiner".to_vec(),
        message_id: [0xdf; 32],
        predecessor_message_id: Some([0xe0; 32]),
        payload: b"complete".to_vec(),
        superseded: false,
    };

    assert!(matches!(
        owner
            .joiner_activate(previous_id, &complete, b"completion")
            .await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn superseded_late_commit_is_recorded_through_the_protocol_entry() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xc1; 16]);
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let mut deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "device-1",
        Vec::new(),
    );
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceConvergence::new(deps);
    let commit = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Commit,
        b"device-1",
        Some([0xc2; 32]),
        b"late-commit",
    );
    let frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *previous_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Commit,
        message_id: commit.message_id,
        predecessor_message_id: commit.predecessor_message_id,
        payload: commit.payload,
    };

    assert!(matches!(
        owner.apply_joiner_commit(&frame, &FailingPreparation).await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
    let previous = repository.load(previous_id).await.unwrap().unwrap();
    assert!(previous
        .inbox_dedup
        .iter()
        .any(|record| record.message_id == frame.message_id));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn superseded_late_complete_is_recorded_through_the_protocol_entry() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xc3; 16]);
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let mut deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "device-1",
        Vec::new(),
    );
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceConvergence::new(deps);
    let complete = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Complete,
        b"device-1",
        Some([0xc4; 32]),
        b"late-complete",
    );
    let frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *previous_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Complete,
        message_id: complete.message_id,
        predecessor_message_id: complete.predecessor_message_id,
        payload: complete.payload,
    };

    assert!(matches!(
        owner.activate_joiner_complete(&frame).await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
    let previous = repository.load(previous_id).await.unwrap().unwrap();
    assert!(previous
        .inbox_dedup
        .iter()
        .any(|record| record.message_id == frame.message_id));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn superseded_rejection_only_confirms_old_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xdb; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let rejected = uc_core::membership::AdmissionOutboxMessageV1 {
        purpose: uc_core::membership::AdmissionOutboxPurposeV1::Rejected,
        recipient: b"joiner".to_vec(),
        message_id: [0xdc; 32],
        predecessor_message_id: Some([0xd3; 32]),
        payload: b"cleanup-confirmed".to_vec(),
        superseded: false,
    };

    owner
        .joiner_record_rejected(previous_id, &rejected)
        .await
        .unwrap();

    let previous = repository.load(previous_id).await.unwrap().unwrap();
    assert_eq!(
        previous.terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
    );
    assert_eq!(previous.rejection_reason, None);
    assert!(previous.outboxes.iter().all(|message| message.superseded));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn recovery_keeps_current_join_and_old_cleanup_isolated() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xdd; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;

    let report = owner
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();

    assert_eq!(report.deliveries_attempted, 2);
    assert_eq!(report.deliveries_confirmed, 2);
    assert_eq!(report.attempts_compacted, 1);
    assert!(repository.load(previous_id).await.unwrap().is_none());
    assert_eq!(
        repository
            .load_terminal(previous_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin
    );
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current_id
    );
}

#[tokio::test]
async fn recovery_handles_multiple_superseded_cleanups_with_one_current_join() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe1; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"first-request",
            false,
        )
        .await
        .unwrap();
    let second = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"second-sponsor",
            b"second-request",
            false,
        )
        .await
        .unwrap();
    let current = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"third-sponsor",
            b"third-request",
            false,
        )
        .await
        .unwrap();

    let report = owner
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();

    assert_eq!(report.deliveries_attempted, 3);
    assert_eq!(report.deliveries_confirmed, 3);
    assert_eq!(report.attempts_compacted, 2);
    assert!(repository
        .load_terminal(first.attempt.attempt_id)
        .await
        .unwrap()
        .is_some());
    assert!(repository
        .load_terminal(second.attempt.attempt_id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        current.attempt.attempt_id
    );
}

#[test]
fn durable_admission_preparation_rejects_security_result_mismatch() {
    let (history, candidate_event, commitment) = admission_verification_fixture([0x84; 32]);
    let mut different = commitment.clone();
    different.security_commitment_id[0] ^= 0xff;

    let result = super::admission::verify_candidate_preparation(
        history,
        &candidate_event,
        &commitment,
        &different,
        &DeterministicHistoricalVerifier,
    );

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
}

#[tokio::test]
async fn pending_member_removal_before_commit_rejects_without_add() {
    use super::admission::PendingMemberRemovalOutcomeV1;
    use uc_core::membership::{AdmissionRejectionReasonV1, VersionedMembershipHistory};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x65; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x66; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x67; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x68; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x69; 32],
            &initiated.outboxes[0],
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let removal = durable_candidate_removal_fixture(attempt_id);

    let outcome = sponsor
        .sponsor_remove_pending_member(
            attempt_id,
            &removal,
            b"joiner",
            b"removed-before-activation",
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        PendingMemberRemovalOutcomeV1::AdmissionRejected(_)
    ));
    let attempt = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        attempt.rejection_reason,
        Some(AdmissionRejectionReasonV1::RemovedBeforeActivation)
    );
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
}

#[tokio::test]
async fn sponsor_business_rejection_before_commit_is_durable_and_replayable() {
    use uc_core::membership::{
        AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1, VersionedMembershipHistory,
    };

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x6f; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x70; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x71; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x72; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x73; 32],
            &initiated.outboxes[0],
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();

    let rejected = sponsor
        .sponsor_reject_before_commit(
            attempt_id,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();
    let replayed = sponsor
        .sponsor_reject_before_commit(
            attempt_id,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();

    assert_eq!(rejected, replayed);
    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);
    let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.rejection_reason,
        Some(AdmissionRejectionReasonV1::IdentityConflict)
    );
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
}

#[tokio::test]
async fn pending_member_removal_after_commit_permanently_keeps_add_then_remove() {
    use super::admission::PendingMemberRemovalOutcomeV1;
    use uc_core::membership::{AdmissionRejectionReasonV1, VersionedMembershipHistory};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x6a; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x6b; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x6c; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x6d; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let candidate_message = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x6e; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let removal = durable_candidate_removal_fixture(attempt_id);

    let outcome = sponsor
        .sponsor_remove_pending_member(
            attempt_id,
            &removal,
            b"joiner",
            b"removed-before-activation",
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        PendingMemberRemovalOutcomeV1::AdmissionRejected(_)
    ));
    let attempt = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        attempt.rejection_reason,
        Some(AdmissionRejectionReasonV1::RemovedBeforeActivation)
    );
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
    assert_eq!(history.active_members().len(), 1);
    assert_eq!(history.depth(removal.event_id()), Some(9));
}

#[tokio::test]
async fn pending_member_removal_races_commit_and_activation_without_partial_state() {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, SponsorAdmissionStageV1, SponsorAdmissionStateV1,
        VersionedMembershipHistory,
    };

    for iteration in 0..8u8 {
        let sponsor_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xa0 | iteration; 16]);
        let joiner_repository = durable_admission_repository(&joiner_dir, [0xb0 | iteration; 16]);
        let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
        let joiner = durable_admission_owner(joiner_repository);
        let attempt_id =
            uc_core::membership::AdmissionAttemptId::from_bytes([0xc0 | iteration; 32]);
        let initiated = joiner
            .start_join(
                attempt_id,
                [0xd0 | iteration; 16],
                b"sponsor",
                b"join-request",
                b"joiner-pending-state",
                b"joiner-key-package",
                b"joiner-target-access",
            )
            .await
            .unwrap();
        let (candidate, base_history, candidate_event, commitment, _) =
            durable_candidate_verification_fixture(attempt_id);
        let offered = sponsor
            .sponsor_accept_and_offer(
                attempt_id,
                [0xe0 | iteration; 32],
                &initiated.outboxes[0],
                candidate.clone(),
                base_history.clone(),
                &candidate_event,
                &commitment,
                b"joiner",
                b"candidate",
            )
            .await
            .unwrap();
        let prepared = joiner
            .joiner_verify_and_prepare(
                attempt_id,
                &offered,
                candidate,
                base_history,
                &candidate_event,
                &commitment,
                b"joiner-target-access",
                b"verified-complete-history",
                None,
                b"sponsor",
                b"prepared",
            )
            .await
            .unwrap();
        let removal = durable_candidate_removal_fixture(attempt_id);

        let _ = tokio::join!(
            sponsor.sponsor_commit(
                attempt_id,
                &prepared,
                b"verified-complete-history",
                b"joiner",
                b"commit",
            ),
            sponsor.sponsor_remove_pending_member(
                attempt_id,
                &removal,
                b"joiner",
                b"removed-before-activation",
            )
        );

        let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_repository
                .load_membership_history_v2()
                .await
                .unwrap()
                .unwrap(),
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
        match saved.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected,
            }) => {
                assert_eq!(history.effective_members().len(), 1);
                assert_eq!(history.active_members().len(), 1);
            }
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Committed,
            }) => {
                assert_eq!(history.effective_members().len(), 2);
                assert_eq!(history.active_members().len(), 1);
            }
            other => panic!("unexpected commit/removal race result: {other:?}"),
        }
    }

    for iteration in 0..8u8 {
        let sponsor_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x10 | iteration; 16]);
        let joiner_repository = durable_admission_repository(&joiner_dir, [0x20 | iteration; 16]);
        let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
        let joiner = durable_admission_owner(joiner_repository);
        let attempt_id =
            uc_core::membership::AdmissionAttemptId::from_bytes([0x30 | iteration; 32]);
        let initiated = joiner
            .start_join(
                attempt_id,
                [0x40 | iteration; 16],
                b"sponsor",
                b"join-request",
                b"joiner-pending-state",
                b"joiner-key-package",
                b"joiner-target-access",
            )
            .await
            .unwrap();
        let (candidate, base_history, candidate_event, commitment, receipt) =
            durable_candidate_verification_fixture(attempt_id);
        let offered = sponsor
            .sponsor_accept_and_offer(
                attempt_id,
                [0x50 | iteration; 32],
                &initiated.outboxes[0],
                candidate.clone(),
                base_history.clone(),
                &candidate_event,
                &commitment,
                b"joiner",
                b"candidate",
            )
            .await
            .unwrap();
        let prepared = joiner
            .joiner_verify_and_prepare(
                attempt_id,
                &offered,
                candidate,
                base_history,
                &candidate_event,
                &commitment,
                b"joiner-target-access",
                b"verified-complete-history",
                None,
                b"sponsor",
                b"prepared",
            )
            .await
            .unwrap();
        let commit = sponsor
            .sponsor_commit(
                attempt_id,
                &prepared,
                b"verified-complete-history",
                b"joiner",
                b"commit",
            )
            .await
            .unwrap();
        let applied = joiner
            .joiner_apply(attempt_id, &commit, &receipt, b"sponsor", b"applied")
            .await
            .unwrap();
        let removal = durable_candidate_removal_fixture(attempt_id);

        let _ = tokio::join!(
            sponsor.sponsor_complete(
                attempt_id,
                &applied,
                &receipt,
                b"admission-completion",
                b"joiner",
                b"complete",
            ),
            sponsor.sponsor_remove_pending_member(
                attempt_id,
                &removal,
                b"joiner",
                b"removed-before-activation",
            )
        );

        let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_repository
                .load_membership_history_v2()
                .await
                .unwrap()
                .unwrap(),
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
        match saved.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected,
            }) => {
                assert_eq!(history.effective_members().len(), 1);
                assert_eq!(history.active_members().len(), 1);
            }
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Applied | SponsorAdmissionStageV1::Completed,
            }) => {
                assert_eq!(history.effective_members().len(), 2);
                assert_eq!(history.active_members().len(), 2);
            }
            other => panic!("unexpected activation/removal race result: {other:?}"),
        }
    }
}

#[test]
fn durable_admission_preparation_rejects_unverified_history() {
    let (history, mut candidate_event, commitment) = admission_verification_fixture([0x84; 32]);
    candidate_event.signature[0] ^= 0xff;

    let result = super::admission::verify_candidate_preparation(
        history,
        &candidate_event,
        &commitment,
        &commitment,
        &DeterministicHistoricalVerifier,
    );

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
}

#[tokio::test]
async fn third_member_completion_keeps_joiner_pending_until_helper_applies_its_update() {
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use uc_core::membership::{
        AdmissionActivationReceipt, AdmissionAttemptV1, AdmissionIdentityBindingV1,
        AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, MembershipActivationBaselineV2, MembershipAdmissionV2,
        MembershipCredential, MembershipEventId, MembershipEventV2, MembershipOperationV2,
        SponsorAdmissionSecurityDelivery, VersionedMembershipHistory,
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
        MEMBERSHIP_EVENT_FORMAT_V2,
    };

    let verifier = DeterministicHistoricalVerifier;
    let sponsor_device = DeviceId::new("sponsor");
    let helper_device = DeviceId::new("helper");
    let joiner_device = DeviceId::new("joiner");
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xc1; 32]);
    let helper_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xc2; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xc3; 32]);
    let sponsor_instance = sponsor_credential.member_instance_id(&sponsor_device);
    let helper_instance = helper_credential.member_instance_id(&helper_device);
    let joiner_instance = joiner_credential.member_instance_id(&joiner_device);
    let mut sponsor_facts = admission_facts_for(sponsor_instance, &sponsor_device);
    sponsor_facts.identity_signature =
        verifier.sign(&sponsor_credential, &sponsor_facts.signing_payload());
    let mut helper_facts = admission_facts_for(helper_instance, &helper_device);
    helper_facts.transport_public_key = vec![0x35; 32];
    helper_facts.transport_address_blob = b"helper-recovery-route".to_vec();
    helper_facts.identity_signature =
        verifier.sign(&helper_credential, &helper_facts.signing_payload());
    let base_head = MembershipEventId::from_hex(&"c4".repeat(32)).unwrap();
    let base_history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::FullyVerifiedMigration {
            lineage_id: SPACE.to_owned(),
            head_event_id: base_head,
            head_depth: 4,
            current_members: vec![
                (sponsor_facts.clone(), sponsor_credential.clone()),
                (helper_facts.clone(), helper_credential.clone()),
            ],
        },
    )
    .unwrap();
    let base_position = base_history.current_position().unwrap();
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc5; 32]);
    let resume_private = [0xc6; 32];
    let resume_public = SigningKey::from_bytes(&resume_private)
        .verifying_key()
        .to_bytes()
        .to_vec();
    let key_catalog = admission_key_catalog();
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        SPACE.to_owned(),
        SPACE.as_bytes().to_vec(),
        *attempt_id.as_bytes(),
        base_position.clone(),
        [0xc7; 32],
        1,
        3,
        4,
        [0xc8; 32],
        [0xc9; 32],
        [0xca; 32],
        key_catalog.digest(),
        [0xcb; 32],
    )
    .unwrap();
    let mut joiner_facts = admission_facts_for(joiner_instance, &joiner_device);
    joiner_facts.transport_public_key = vec![0x36; 32];
    joiner_facts.identity_signature =
        verifier.sign(&joiner_credential, &joiner_facts.signing_payload());
    let operation = MembershipOperationV2::AddDevice {
        admission: MembershipAdmissionV2 {
            facts: joiner_facts.clone(),
            membership_credential: joiner_credential.clone(),
            resume_public_key_digest: super::admission::admission_resume_public_key_digest(
                &resume_public,
            ),
            security_commitment_id: commitment.security_commitment_id,
        },
    };
    let resulting_members_digest = base_history
        .expected_resulting_members_digest(Some(base_head), &operation)
        .unwrap();
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        SPACE.to_owned(),
        Some(base_head),
        5,
        [0xcc; 16],
        sponsor_instance,
        sponsor_credential.credential_id,
        sponsor_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [0xcd; 32],
        vec![0xce],
        Some(commitment.admission_bundle_digest),
        Vec::new(),
    );
    event.signature = verifier.sign(&sponsor_credential, &event.signing_payload());
    let mut completed_history = base_history.clone();
    completed_history
        .verify_and_receive_event(event.clone(), &verifier)
        .unwrap();
    let mut receipt = AdmissionActivationReceipt::new(
        1,
        *attempt_id.as_bytes(),
        event.event_id(),
        event.resulting_members_digest,
        commitment.security_commitment_id,
        joiner_instance,
        Vec::new(),
    );
    receipt.signature = verifier.sign(&joiner_credential, &receipt.signing_payload());
    completed_history
        .verify_and_record_activation_receipt(receipt.clone(), &verifier)
        .unwrap();
    let base_history_bytes = base_history.encode_persisted_v2().unwrap();
    let completed_history_bytes = completed_history.encode_persisted_v2().unwrap();
    let event_bytes = postcard::to_stdvec(&event).unwrap();
    let commitment_bytes = postcard::to_stdvec(&commitment).unwrap();
    let receipt_bytes = postcard::to_stdvec(&receipt).unwrap();
    let delivery = SponsorAdmissionSecurityDelivery {
        recipient: helper_device.clone(),
        credential_id: helper_credential.credential_id,
        payload: b"helper-security-update".to_vec(),
    };

    let joiner_directory = tempfile::tempdir().unwrap();
    let joiner_repository = durable_admission_repository(&joiner_directory, [0xcf; 16]);
    let mut joiner_attempt =
        AdmissionAttemptV1::new_joiner(attempt_id, [0xd0; 16], JoinerAdmissionStageV1::Applied);
    joiner_attempt.local_join_ordinal = Some(0);
    joiner_attempt.lineage_id = Some(SPACE.to_owned());
    joiner_attempt.base_history_position = Some(postcard::to_stdvec(&base_position).unwrap());
    joiner_attempt.candidate_event = Some(event_bytes.clone());
    joiner_attempt.candidate_event_id = Some(*event.event_id().as_bytes());
    joiner_attempt.candidate_key_package = Some(b"joiner-key-package".to_vec());
    joiner_attempt.target_members_digest = Some(resulting_members_digest);
    joiner_attempt.security_commitment = Some(commitment_bytes.clone());
    joiner_attempt.security_commit = Some(b"security-commit".to_vec());
    joiner_attempt.security_welcome = Some(b"security-welcome".to_vec());
    joiner_attempt.target_protection_group_id = Some("target-protection-group".to_owned());
    joiner_attempt.target_key_catalog = Some(key_catalog.encode().unwrap());
    joiner_attempt.target_relationships = Some(vec![
        sponsor_facts.clone(),
        helper_facts.clone(),
        joiner_facts.clone(),
    ]);
    joiner_attempt.existing_member_security_deliveries = Some(vec![delivery]);
    joiner_attempt.staged_security_state = Some(b"joiner-staged-security".to_vec());
    joiner_attempt.joiner_pending_security_state = Some(b"joiner-pending-security".to_vec());
    joiner_attempt.base_membership_history = Some(base_history_bytes);
    joiner_attempt.verified_membership_history = Some(completed_history_bytes.clone());
    joiner_attempt.prepared_proof = Some(b"prepared-proof".to_vec());
    joiner_attempt.activation_receipt = Some(receipt_bytes);
    joiner_attempt.resume_public_key = Some(resume_public.clone());
    joiner_attempt.resume_private_key = Some(resume_private.to_vec());
    joiner_attempt.target_access_state = Some(b"target-access".to_vec());
    joiner_attempt.identity_binding = Some(
        AdmissionIdentityBindingV1::new(
            SPACE.to_owned(),
            event.event_id(),
            &sponsor_facts,
            &joiner_facts,
        )
        .unwrap()
        .encode()
        .unwrap(),
    );
    joiner_attempt
        .outboxes
        .push(super::admission::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            b"sponsor",
            Some([0xd1; 32]),
            b"applied",
        ));
    joiner_repository
        .create(&joiner_attempt, None, Some(&completed_history_bytes))
        .await
        .unwrap();

    let helper_directory = tempfile::tempdir().unwrap();
    let helper_repository = durable_admission_repository(&helper_directory, [0xd2; 16]);
    helper_repository
        .compare_and_replace_membership_history_v2(None, &completed_history_bytes)
        .await
        .unwrap();

    let mut joiner_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "joiner",
        Vec::new(),
    );
    joiner_deps.admission_attempts = Arc::clone(&joiner_repository);
    joiner_deps.historical_membership_signatures = Arc::new(DeterministicHistoricalVerifier);
    let joiner = WorkspaceConvergence::new(joiner_deps);

    let mut blocked_helper_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "helper",
        Vec::new(),
    );
    blocked_helper_deps.admission_attempts = Arc::clone(&helper_repository);
    blocked_helper_deps.historical_membership_signatures =
        Arc::new(DeterministicHistoricalVerifier);
    blocked_helper_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: helper_device.clone(),
        credential: helper_credential.clone(),
    });
    blocked_helper_deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: helper_device.clone(),
    });
    let blocked_helper = WorkspaceConvergence::new(blocked_helper_deps);

    let hello = joiner
        .prepare_completion_recovery_hello(*attempt_id.as_bytes(), helper_instance)
        .await
        .unwrap();
    let transport_binding = uc_core::membership::AdmissionCompletionRecoveryTransportBindingV1 {
        joiner_transport_identity_digest: Sha256::digest(&joiner_facts.transport_public_key).into(),
        helper_transport_identity_digest: Sha256::digest(&helper_facts.transport_public_key).into(),
    };
    let joiner_applied_message_id = joiner_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap()
        .outboxes
        .iter()
        .find(|message| message.purpose == AdmissionOutboxPurposeV1::Applied)
        .unwrap()
        .message_id;
    let mut changed_transport_binding = transport_binding;
    changed_transport_binding.helper_transport_identity_digest = [0xff; 32];
    assert!(blocked_helper
        .challenge_completion_recovery(
            &hello,
            changed_transport_binding,
            joiner_applied_message_id,
            [0xd4; 32],
        )
        .await
        .is_err());
    let challenge = blocked_helper
        .challenge_completion_recovery(
            &hello,
            transport_binding,
            joiner_applied_message_id,
            [0xd4; 32],
        )
        .await
        .unwrap();
    let response = joiner
        .respond_to_completion_recovery(&hello, &challenge)
        .await
        .unwrap();

    assert!(blocked_helper
        .complete_recovered_admission(&hello, &response)
        .await
        .is_err());
    let blocked = helper_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(blocked.stage_rank(), Some(5));
    assert_eq!(blocked.terminal_result, None);

    let helper_activation = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut resumed_helper_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "helper",
        Vec::new(),
    );
    resumed_helper_deps.admission_attempts = Arc::clone(&helper_repository);
    resumed_helper_deps.historical_membership_signatures =
        Arc::new(DeterministicHistoricalVerifier);
    resumed_helper_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: helper_device.clone(),
        credential: helper_credential,
    });
    resumed_helper_deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: helper_device,
    });
    resumed_helper_deps.activate_completion_helper_admission_security = helper_activation.clone();
    let resumed_helper = WorkspaceConvergence::new(resumed_helper_deps);
    let complete = resumed_helper
        .complete_recovered_admission(&hello, &response)
        .await
        .unwrap();
    let replayed_complete = resumed_helper
        .complete_recovered_admission(&hello, &response)
        .await
        .unwrap();
    assert_eq!(replayed_complete, complete);
    assert_eq!(
        helper_activation
            .helper_activation_requests
            .lock()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        helper_repository
            .load(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        Some(AdmissionTerminalResultV1::Completed)
    );

    assert!(matches!(
        joiner.activate_joiner_complete(&complete).await.unwrap(),
        crate::space::admission::adapter::DurableJoinerCompletion::Active(_)
    ));
    assert_eq!(
        joiner_repository
            .load_terminal(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        AdmissionTerminalResultV1::Active
    );
}

#[tokio::test]
async fn explicit_sponsor_rejection_ends_a_join_before_candidate() {
    use uc_core::membership::{AdmissionRejectionReasonV1, AdmissionTerminalResultV1};

    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x4d; 16]);
    let joiner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x4e; 32]);
    joiner
        .start_join(
            attempt_id,
            [0x4f; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();

    joiner
        .joiner_reject_before_candidate(attempt_id, AdmissionRejectionReasonV1::HistoryConflict)
        .await
        .unwrap();
    joiner
        .joiner_reject_before_candidate(attempt_id, AdmissionRejectionReasonV1::HistoryConflict)
        .await
        .unwrap();

    let rejected = repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        rejected.terminal_result,
        Some(AdmissionTerminalResultV1::Rejected)
    );
    assert_eq!(
        rejected.rejection_reason,
        Some(AdmissionRejectionReasonV1::HistoryConflict)
    );
    assert!(rejected.joiner_pending_security_state.is_none());
    assert!(rejected.outboxes.iter().all(|message| message.superseded));
}

#[tokio::test]
async fn durable_admission_cancel_and_commit_have_exactly_one_winner() {
    use uc_core::membership::AdmissionOutboxPurposeV1;
    async fn prepared_pair(
        sponsor: &super::admission::DurableAdmissionTransaction,
        joiner: &super::admission::DurableAdmissionTransaction,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        join_id: [u8; 16],
    ) -> uc_core::membership::AdmissionOutboxMessageV1 {
        let initiated = joiner
            .start_join(
                attempt_id,
                join_id,
                b"sponsor",
                b"join-request",
                b"joiner-pending-state",
                b"joiner-key-package",
                b"joiner-target-access",
            )
            .await
            .unwrap();
        let (candidate, base_history, candidate_event, commitment, _activation_receipt) =
            durable_candidate_verification_fixture(attempt_id);
        let offered = sponsor
            .sponsor_accept_and_offer(
                attempt_id,
                [0x53; 32],
                &initiated.outboxes[0],
                candidate.clone(),
                base_history.clone(),
                &candidate_event,
                &commitment,
                b"joiner",
                b"candidate",
            )
            .await
            .unwrap();
        sponsor
            .record_invitation_consume_result(
                attempt_id,
                super::admission::InvitationConsumeResultV1::NotFound,
            )
            .await
            .unwrap();
        joiner
            .joiner_verify_and_prepare(
                attempt_id,
                &offered,
                candidate,
                base_history,
                &candidate_event,
                &commitment,
                b"joiner-target-access",
                b"verified-complete-history",
                None,
                b"sponsor",
                b"prepared",
            )
            .await
            .unwrap()
    }

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x54; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x55; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x56; 32]);
    let prepared = prepared_pair(&sponsor, &joiner, attempt_id, [0x57; 16]).await;
    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);
    let sponsor_rejected = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(sponsor_rejected.candidate_event.is_some());
    assert!(sponsor_rejected.activation_receipt.is_none());
    assert!(sponsor_rejected.terminal_result.is_some());
    assert!(sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit"
        )
        .await
        .is_err());
    let rejected_ack = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    let replayed_rejected_ack = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    assert_eq!(replayed_rejected_ack, rejected_ack);
    sponsor
        .sponsor_confirm_rejected(attempt_id, &rejected_ack)
        .await
        .unwrap();
    let joiner_rejected = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        joiner_rejected.terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::Rejected)
    );
    assert_eq!(
        joiner_rejected.rejection_reason,
        Some(uc_core::membership::AdmissionRejectionReasonV1::Cancelled)
    );
    assert!(joiner_rejected
        .outboxes
        .iter()
        .all(|message| message.superseded));
    let sponsor_rejected = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(sponsor_rejected
        .outboxes
        .iter()
        .all(|message| message.superseded));
    joiner.compact_if_settled(attempt_id).await.unwrap();
    sponsor.compact_if_settled(attempt_id).await.unwrap();

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x58; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x59; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x5a; 32]);
    let prepared = prepared_pair(&sponsor, &joiner, attempt_id, [0x5b; 16]).await;
    let committed = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let still_committed = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(still_committed, committed);
    let activation_receipt = durable_candidate_verification_fixture(attempt_id).4;
    let applied = joiner
        .joiner_apply(
            attempt_id,
            &committed,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    let joiner_state = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        joiner_state.cancel_outcome,
        Some(b"too_late_committed".to_vec())
    );
    assert_eq!(applied.purpose, AdmissionOutboxPurposeV1::Applied);
}

#[tokio::test]
async fn base_history_change_after_candidate_is_durably_rejected_without_add() {
    use uc_core::membership::{AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x60; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x61; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x62; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x63; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let candidate_message = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x64; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared_message = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();

    let mut concurrent = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let expected_version = concurrent.record_version;
    concurrent.record_version += 1;
    let current_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    sponsor_repository
        .compare_and_advance_with_membership_history_v2(
            attempt_id,
            expected_version,
            &concurrent,
            Some(&current_history),
            b"newer-formal-history",
        )
        .await
        .unwrap();

    let rejected = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared_message,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();

    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);
    let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.rejection_reason,
        Some(AdmissionRejectionReasonV1::BaseHistoryChanged)
    );
    assert_eq!(
        sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(b"newer-formal-history".to_vec())
    );
}

#[tokio::test]
async fn base_history_change_during_commit_is_durably_rejected() {
    use uc_core::membership::{AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x5c; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x5d; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x5e; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x5f; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x60; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let racing_repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort> =
        Arc::new(HistoryRaceAdmissionRepository {
            inner: Arc::clone(&sponsor_repository),
            inject_once: AtomicBool::new(true),
            replacement_history: b"concurrent-formal-history".to_vec(),
        });
    let racing_sponsor = durable_admission_owner(racing_repository);

    let result = racing_sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();

    assert_eq!(result.purpose, AdmissionOutboxPurposeV1::Rejected);
    let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.rejection_reason,
        Some(AdmissionRejectionReasonV1::BaseHistoryChanged)
    );
    assert_eq!(
        sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(b"concurrent-formal-history".to_vec())
    );
}

#[tokio::test]
async fn durable_admission_becomes_complete_only_after_both_sides_save() {
    use uc_core::membership::AdmissionTerminalResultV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x41; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x42; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x43; 32]);
    let join_id = [0x44; 16];
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);

    let initiated = joiner
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    assert_eq!(
        initiated.target_access_state.as_deref(),
        Some(b"joiner-target-access".as_slice())
    );
    let candidate_message = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x47; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let replayed_candidate = durable_admission_owner(Arc::clone(&sponsor_repository))
        .sponsor_accept_and_offer(
            attempt_id,
            [0x47; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    assert_eq!(replayed_candidate, candidate_message);
    let sponsor_candidate_state = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(sponsor_candidate_state.outboxes.iter().any(|message| {
        message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::InvitationConsume
            && !message.superseded
    }));
    sponsor
        .record_invitation_consume_result(
            attempt_id,
            super::admission::InvitationConsumeResultV1::Retryable,
        )
        .await
        .unwrap();
    assert!(sponsor_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap()
        .outboxes
        .iter()
        .any(|message| {
            message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::InvitationConsume
                && !message.superseded
        }));
    sponsor
        .record_invitation_consume_result(
            attempt_id,
            super::admission::InvitationConsumeResultV1::Consumed,
        )
        .await
        .unwrap();
    let prepared_message = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let replayed_prepared = durable_admission_owner(Arc::clone(&joiner_repository))
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    assert_eq!(replayed_prepared, prepared_message);
    let commit_message = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared_message,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let replayed_commit = durable_admission_owner(Arc::clone(&sponsor_repository))
        .sponsor_commit(
            attempt_id,
            &prepared_message,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    assert_eq!(replayed_commit, commit_message);
    let sponsor_committed_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let sponsor_committed_history =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_committed_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(sponsor_committed_history.effective_members().len(), 2);
    assert_eq!(sponsor_committed_history.active_members().len(), 1);
    let applied_message = joiner
        .joiner_apply(
            attempt_id,
            &commit_message,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    let replayed_applied = durable_admission_owner(Arc::clone(&joiner_repository))
        .joiner_apply(
            attempt_id,
            &commit_message,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    assert_eq!(replayed_applied, applied_message);
    let joiner_applied_history = joiner_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let joiner_applied_history =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &joiner_applied_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(joiner_applied_history.active_members().len(), 2);
    let complete_message = sponsor
        .sponsor_complete(
            attempt_id,
            &applied_message,
            &activation_receipt,
            b"admission-completion",
            b"joiner",
            b"complete",
        )
        .await
        .unwrap();
    let security_update = sponsor
        .enqueue_post_commit_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            b"existing-member",
            b"event-epoch-and-security-commitment",
        )
        .await
        .unwrap();
    let other_security_update = sponsor
        .enqueue_post_commit_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            b"other-existing-member",
            b"event-epoch-and-security-commitment",
        )
        .await
        .unwrap();
    assert_ne!(security_update.message_id, other_security_update.message_id);
    assert_ne!(security_update.recipient, other_security_update.recipient);
    let history_batch = sponsor
        .enqueue_post_commit_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::HistoryOrReceiptBatch,
            b"existing-member",
            b"history-page-and-receipt-ids",
        )
        .await
        .unwrap();
    let security_ack = super::admission::admission_acknowledgment(&security_update);
    assert!(sponsor
        .acknowledge_delivery(attempt_id, &security_ack)
        .await
        .is_err());
    sponsor
        .acknowledge_persisted_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            &security_ack,
        )
        .await
        .unwrap();
    let after_exact_security_ack = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(after_exact_security_ack
        .outboxes
        .iter()
        .any(|message| { message.message_id == security_update.message_id && message.superseded }));
    assert!(after_exact_security_ack.outboxes.iter().any(|message| {
        message.message_id == other_security_update.message_id && !message.superseded
    }));
    assert!(after_exact_security_ack.outboxes.iter().any(|message| {
        message.message_id == complete_message.message_id && !message.superseded
    }));
    sponsor
        .acknowledge_persisted_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::HistoryOrReceiptBatch,
            &super::admission::admission_acknowledgment(&history_batch),
        )
        .await
        .unwrap();
    let replayed_complete = durable_admission_owner(Arc::clone(&sponsor_repository))
        .sponsor_complete(
            attempt_id,
            &applied_message,
            &activation_receipt,
            b"admission-completion",
            b"joiner",
            b"complete",
        )
        .await
        .unwrap();
    assert_eq!(replayed_complete, complete_message);
    let sponsor_applied_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let sponsor_applied_history =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_applied_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(sponsor_applied_history.active_members().len(), 2);

    let sponsor_after_complete = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let ordinary_removal = sponsor
        .sponsor_remove_pending_member(
            attempt_id,
            &durable_candidate_removal_fixture(attempt_id),
            b"joiner",
            b"removed-before-activation",
        )
        .await
        .unwrap();
    assert!(matches!(
        ordinary_removal,
        super::admission::PendingMemberRemovalOutcomeV1::OrdinaryMemberRemovalRequired
    ));
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(sponsor_after_complete)
    );

    let sponsor_before_ack = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        sponsor_before_ack.terminal_result,
        Some(AdmissionTerminalResultV1::Completed)
    );
    assert_eq!(
        joiner
            .joiner_activate(attempt_id, &complete_message, b"admission-completion")
            .await
            .unwrap(),
        super::admission::JoinerActivationOutcomeV1::SpaceTransitionRequired
    );
    let restarted_joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    assert!(restarted_joiner
        .requires_session_transition()
        .await
        .unwrap());
    assert_eq!(
        restarted_joiner
            .recover_space_transitions_after_session_drain()
            .await
            .unwrap(),
        1
    );
    assert!(!restarted_joiner
        .requires_session_transition()
        .await
        .unwrap());
    let complete_ack = match restarted_joiner
        .joiner_activate(attempt_id, &complete_message, b"admission-completion")
        .await
        .unwrap()
    {
        super::admission::JoinerActivationOutcomeV1::Active(acknowledgment) => acknowledgment,
        super::admission::JoinerActivationOutcomeV1::SpaceTransitionRequired => {
            panic!("completed activation must rebuild its acknowledgment")
        }
    };
    let replayed_ack = durable_admission_owner(Arc::clone(&joiner_repository))
        .joiner_activate(attempt_id, &complete_message, b"admission-completion")
        .await
        .unwrap();
    assert_eq!(
        replayed_ack,
        super::admission::JoinerActivationOutcomeV1::Active(complete_ack.clone())
    );
    assert!(joiner_repository.load(attempt_id).await.unwrap().is_none());
    let joiner_saved = joiner_repository
        .load_terminal(attempt_id)
        .await
        .unwrap()
        .unwrap();
    let joiner_history = joiner_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let verified_history: uc_core::membership::VersionedMembershipHistory =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &joiner_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(verified_history.effective_members().len(), 2);
    assert_eq!(
        joiner_saved.terminal_result,
        AdmissionTerminalResultV1::Active
    );

    sponsor
        .sponsor_confirm_active(attempt_id, &complete_ack)
        .await
        .unwrap();
    sponsor
        .sponsor_confirm_active(attempt_id, &complete_ack)
        .await
        .unwrap();
    let sponsor_saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        sponsor_saved.terminal_result,
        Some(AdmissionTerminalResultV1::Completed)
    );
    assert_eq!(
        sponsor_saved.activation_receipt,
        Some(postcard::to_stdvec(&activation_receipt).unwrap())
    );
    assert_eq!(sponsor_saved.completion, Some(joiner_saved.replay_result));
    assert!(sponsor_saved.outboxes.iter().any(|message| {
        message.message_id == other_security_update.message_id && !message.superseded
    }));
    assert!(sponsor.compact_if_settled(attempt_id).await.is_err());
    sponsor
        .acknowledge_persisted_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            &super::admission::admission_acknowledgment(&other_security_update),
        )
        .await
        .unwrap();

    let sponsor_terminal = sponsor.compact_if_settled(attempt_id).await.unwrap();
    let joiner_terminal = joiner.compact_if_settled(attempt_id).await.unwrap();
    assert_eq!(
        sponsor_terminal.terminal_result,
        AdmissionTerminalResultV1::Completed
    );
    assert_eq!(
        joiner_terminal.terminal_result,
        AdmissionTerminalResultV1::Active
    );
    assert!(sponsor_repository.load(attempt_id).await.unwrap().is_none());
    assert!(joiner_repository.load(attempt_id).await.unwrap().is_none());
    assert!(matches!(
        durable_admission_owner(Arc::clone(&joiner_repository))
            .current_local_join()
            .await
            .unwrap(),
        Some(super::CurrentJoinStatus::Active {
            join_id: projected_join_id,
            joined_space,
        }) if projected_join_id == join_id
            && joined_space.sponsor_device_id.as_str() == "sponsor"
            && joined_space.space_id == candidate_event.lineage_id
            && joined_space.self_device_id.as_str() == "joiner"
            && joined_space.migrated_records.is_none()
            && joined_space.preserved_unreadable_records.is_none()
    ));
    assert_eq!(
        sponsor_repository
            .load_terminal(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        AdmissionTerminalResultV1::Completed
    );
    durable_admission_owner(Arc::clone(&sponsor_repository))
        .sponsor_confirm_active(attempt_id, &complete_ack)
        .await
        .unwrap();
}

#[tokio::test]
async fn out_of_order_durable_messages_leave_the_saved_stage_unchanged() {
    use uc_core::membership::AdmissionOutboxPurposeV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xc1; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0xc2; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc3; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0xc4; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, history, event, commitment, receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let fake_commit = super::admission::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::Commit,
        b"joiner",
        Some([0xc5; 32]),
        b"early-commit",
    );
    let fake_complete = super::admission::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::Complete,
        b"joiner",
        Some([0xc6; 32]),
        b"early-complete",
    );
    let joiner_before = joiner_repository.load(attempt_id).await.unwrap().unwrap();

    assert!(joiner
        .joiner_apply(attempt_id, &fake_commit, &receipt, b"sponsor", b"applied")
        .await
        .is_err());
    assert!(joiner
        .joiner_activate(attempt_id, &fake_complete, b"completion")
        .await
        .is_err());
    assert_eq!(
        joiner_repository.load(attempt_id).await.unwrap(),
        Some(joiner_before)
    );

    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0xc7; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let sponsor_before = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let fake_applied = super::admission::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::Applied,
        b"sponsor",
        Some([0xc8; 32]),
        b"early-applied",
    );

    assert!(sponsor
        .sponsor_complete(
            attempt_id,
            &fake_applied,
            &receipt,
            b"completion",
            b"joiner",
            b"complete",
        )
        .await
        .is_err());
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(sponsor_before)
    );
}

#[tokio::test]
async fn cross_space_activation_saves_complete_before_forward_only_recovery() {
    use uc_core::membership::{
        AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2, AdmissionTerminalResultV1,
        CrossSpaceTransitionPhaseV2,
    };

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xc5; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0xc6; 16]);
    let transition = Arc::new(SimulatedAdmissionSpaceTransition::new_with_phase_failures());
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner_with_space_transition(
        Arc::clone(&joiner_repository),
        transition.clone(),
    );
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc7; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0xc8; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0xc9; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"prepared-proof",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let prepared_attempt = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    let AdmissionSpaceTransitionV2::CrossSpace(prepared_transition) =
        AdmissionSpaceTransitionV2::decode(prepared_attempt.space_transition.as_deref().unwrap())
            .unwrap()
    else {
        panic!("expected a cross-space transition");
    };
    assert_eq!(
        prepared_transition.phase,
        CrossSpaceTransitionPhaseV2::TargetStaged
    );

    let commit = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"prepared-proof",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let applied = joiner
        .joiner_apply(
            attempt_id,
            &commit,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    let complete = sponsor
        .sponsor_complete(
            attempt_id,
            &applied,
            &activation_receipt,
            b"completion",
            b"joiner",
            b"completion",
        )
        .await
        .unwrap();

    assert!(matches!(
        joiner
            .joiner_activate(attempt_id, &complete, b"completion")
            .await
            .unwrap(),
        super::admission::JoinerActivationOutcomeV1::SpaceTransitionRequired
    ));
    assert!(transition.advances.lock().unwrap().is_empty());
    assert!(joiner.requires_session_transition().await.unwrap());
    let interrupted = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        interrupted.completion.as_deref(),
        Some(b"completion".as_slice())
    );
    assert_eq!(interrupted.terminal_result, None);
    assert_eq!(
        match AdmissionSpaceTransitionV2::decode(interrupted.space_transition.as_deref().unwrap(),)
            .unwrap()
        {
            AdmissionSpaceTransitionV2::CrossSpace(transition) => transition.phase,
            _ => panic!("expected a cross-space transition"),
        },
        CrossSpaceTransitionPhaseV2::TargetStaged
    );

    for expected_phase in [
        CrossSpaceTransitionPhaseV2::TargetStaged,
        CrossSpaceTransitionPhaseV2::ActivationStarted,
        CrossSpaceTransitionPhaseV2::SourceFinalized,
        CrossSpaceTransitionPhaseV2::DataRewrapped,
        CrossSpaceTransitionPhaseV2::TargetPromoted,
        CrossSpaceTransitionPhaseV2::CleanupPending,
    ] {
        assert!(joiner
            .recover_space_transitions_after_session_drain()
            .await
            .is_err());
        let saved = joiner_repository.load(attempt_id).await.unwrap().unwrap();
        assert_eq!(saved.terminal_result, None);
        assert_eq!(
            match AdmissionSpaceTransitionV2::decode(saved.space_transition.as_deref().unwrap(),)
                .unwrap()
            {
                AdmissionSpaceTransitionV2::CrossSpace(transition) => transition.phase,
                _ => panic!("expected a cross-space transition"),
            },
            expected_phase
        );
    }

    let transitions_finished = joiner
        .recover_space_transitions_after_session_drain()
        .await
        .unwrap();
    assert_eq!(transitions_finished, 1);
    assert!(!joiner.requires_session_transition().await.unwrap());
    let recovery = joiner
        .recover_with(&DeferredAdmissionDelivery)
        .await
        .unwrap();
    assert_eq!(recovery.deliveries_confirmed, 0);
    assert_eq!(recovery.attempts_compacted, 0);
    assert!(joiner_repository.load(attempt_id).await.unwrap().is_none());
    let active = joiner_repository
        .load_terminal(attempt_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.terminal_result, AdmissionTerminalResultV1::Active);
    let AdmissionSpaceTransitionResultV2::CrossSpace(result) =
        AdmissionSpaceTransitionResultV2::decode(
            active.space_transition_result.as_deref().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected a cross-space result");
    };
    let acknowledgment = match joiner
        .joiner_activate(attempt_id, &complete, b"completion")
        .await
        .unwrap()
    {
        super::admission::JoinerActivationOutcomeV1::Active(acknowledgment) => acknowledgment,
        super::admission::JoinerActivationOutcomeV1::SpaceTransitionRequired => {
            panic!("compacted active admission must rebuild its acknowledgment")
        }
    };
    assert!(active.acknowledgment_rebuild.contains(&acknowledgment));
    let profile = super::ProfileWorkspaceConvergence::new(
        Arc::clone(&joiner_repository),
        DeviceId::new("joiner"),
        Arc::new(UnusedClock),
    );
    let pending_ack = profile
        .pending_joiner_complete_ack()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending_ack.sponsor_device_id, DeviceId::new("sponsor"));
    assert_eq!(
        pending_ack.frame.kind,
        uc_core::pairing::DurableAdmissionMessageKind::CompleteAck
    );
    assert_eq!(
        pending_ack.frame.predecessor_message_id,
        Some(acknowledgment.message_id)
    );
    assert_eq!(result.migrated_records, 3);
    assert_eq!(result.preserved_unreadable_records, 1);
}

#[tokio::test]
async fn cross_space_rejection_discards_target_only_before_activation() {
    use uc_core::membership::{AdmissionSpaceTransitionV2, AdmissionTerminalResultV1};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xd1; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0xd2; 16]);
    let transition = Arc::new(SimulatedAdmissionSpaceTransition::new_with_phase_failures());
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner_with_space_transition(
        Arc::clone(&joiner_repository),
        transition.clone(),
    );
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xd3; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0xd4; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0xd5; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"prepared-proof",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    assert!(matches!(
        AdmissionSpaceTransitionV2::decode(
            joiner_repository
                .load(attempt_id)
                .await
                .unwrap()
                .unwrap()
                .space_transition
                .as_deref()
                .unwrap()
        ),
        Some(AdmissionSpaceTransitionV2::CrossSpace(_))
    ));

    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    let acknowledgment = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    let saved = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.terminal_result,
        Some(AdmissionTerminalResultV1::Rejected)
    );
    assert!(saved.space_transition.is_none());
    assert!(saved.space_transition_result.is_none());
    assert!(saved.inbox_dedup.contains(&acknowledgment));
    assert_eq!(transition.discards.load(Ordering::SeqCst), 1);
    assert_eq!(
        prepared.purpose,
        uc_core::membership::AdmissionOutboxPurposeV1::Prepared
    );
}

#[tokio::test]
async fn invitation_consume_retry_is_no_write_and_terminal_compaction_waits_for_resolution() {
    use super::admission::InvitationConsumeResultV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x2e; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x2f; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x30; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x31; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, history, event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x32; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();

    let before_retry = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let metadata_before_retry = sponsor_repository.profile_metadata().await.unwrap();
    sponsor
        .record_invitation_consume_result(attempt_id, InvitationConsumeResultV1::Retryable)
        .await
        .unwrap();
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(before_retry)
    );
    assert_eq!(
        sponsor_repository.profile_metadata().await.unwrap(),
        metadata_before_retry
    );

    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"rejected")
        .await
        .unwrap();
    let rejected_ack = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    sponsor
        .sponsor_confirm_rejected(attempt_id, &rejected_ack)
        .await
        .unwrap();

    assert!(sponsor.compact_if_settled(attempt_id).await.is_err());
    sponsor
        .record_invitation_consume_result(attempt_id, InvitationConsumeResultV1::Conflict)
        .await
        .unwrap();
    let after_conflict = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let metadata_after_conflict = sponsor_repository.profile_metadata().await.unwrap();
    sponsor
        .record_invitation_consume_result(attempt_id, InvitationConsumeResultV1::Conflict)
        .await
        .unwrap();
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(after_conflict)
    );
    assert_eq!(
        sponsor_repository.profile_metadata().await.unwrap(),
        metadata_after_conflict
    );
    sponsor.compact_if_settled(attempt_id).await.unwrap();
}

#[tokio::test]
async fn restart_recovery_delivers_durable_outboxes_and_compacts_settled_terminal_attempts() {
    use uc_core::membership::{AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1};

    let joiner_dir = tempfile::tempdir().unwrap();
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x79; 16]);
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let joiner_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x7a; 32]);
    joiner
        .start_join(
            joiner_attempt,
            [0x7b; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();

    let report = joiner
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();

    assert_eq!(report.deliveries_attempted, 1);
    assert_eq!(report.deliveries_confirmed, 1);
    assert_eq!(report.attempts_compacted, 0);
    let recovered_join = joiner_repository
        .load(joiner_attempt)
        .await
        .unwrap()
        .unwrap();
    assert!(recovered_join.outboxes[0].superseded);
    assert_eq!(recovered_join.inbox_dedup.len(), 1);

    let sponsor_dir = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x7c; 16]);
    let remote_repository = durable_admission_repository(&remote_dir, [0x7d; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let remote = durable_admission_owner(remote_repository);
    let sponsor_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x7e; 32]);
    let initiated = remote
        .start_join(
            sponsor_attempt,
            [0x7f; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, history, event, commitment, _) =
        durable_candidate_verification_fixture(sponsor_attempt);
    sponsor
        .sponsor_accept_and_offer(
            sponsor_attempt,
            [0x80; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_reject_before_commit(
            sponsor_attempt,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();
    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);

    let report = durable_admission_owner(Arc::clone(&sponsor_repository))
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();

    assert_eq!(report.deliveries_attempted, 2);
    assert_eq!(report.deliveries_confirmed, 2);
    assert_eq!(report.attempts_compacted, 1);
    assert!(sponsor_repository
        .load(sponsor_attempt)
        .await
        .unwrap()
        .is_none());
    assert!(sponsor_repository
        .load_terminal(sponsor_attempt)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn candidate_bound_to_another_attempt_leaves_sponsor_state_unchanged() {
    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x33; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x34; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x35; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x36; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (history, event, commitment) = admission_verification_fixture([0x37; 32]);
    let target_relationships = admission_relationships(&event);
    let identity_binding = admission_identity_binding(&event, &target_relationships);
    let candidate = super::admission::DurableAdmissionCandidateV1 {
        lineage_id: event.lineage_id.clone(),
        base_history_position: postcard::to_stdvec(&commitment.base_history_position).unwrap(),
        candidate_event: postcard::to_stdvec(&event).unwrap(),
        candidate_event_id: *event.event_id().as_bytes(),
        candidate_key_package: b"joiner-key-package".to_vec(),
        resume_public_key: vec![0x8d; 32],
        target_members_digest: event.resulting_members_digest,
        security_commitment: postcard::to_stdvec(&commitment).unwrap(),
        security_commit: b"sealed-security-commit".to_vec(),
        security_welcome: postcard::to_stdvec(&commitment).unwrap(),
        target_protection_group_id: "target-protection-group".to_owned(),
        target_key_catalog: admission_key_catalog().encode().unwrap(),
        target_relationships,
        existing_member_deliveries: Vec::new(),
        staged_security_state: b"sponsor-staged-state".to_vec(),
        identity_binding,
    };

    let result = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x38; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
    assert!(sponsor_repository.load(attempt_id).await.unwrap().is_none());
    assert!(sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn activation_receipt_bound_to_another_attempt_leaves_joiner_state_unchanged() {
    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x39; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x3a; 16]);
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x3b; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x3c; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x3d; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let commit = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let before_attempt = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    let before_history = joiner_repository
        .load_membership_history_v2()
        .await
        .unwrap();
    let other_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x3e; 32]);
    let wrong_receipt = durable_candidate_verification_fixture(other_attempt).4;

    let result = joiner
        .joiner_apply(attempt_id, &commit, &wrong_receipt, b"sponsor", b"applied")
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
    assert_eq!(
        joiner_repository.load(attempt_id).await.unwrap(),
        Some(before_attempt)
    );
    assert_eq!(
        joiner_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        before_history
    );
}

#[tokio::test]
async fn durable_join_is_saved_before_the_target_space_is_known() {
    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x3f; 16]);
    let repository = durable_admission_repository(&joiner_dir, [0x40; 16]);
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x41; 32]);

    let initiated = joiner
        .start_join_before_network(
            attempt_id,
            [0x42; 16],
            b"invitation-code",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();

    assert!(initiated.target_access_state.is_none());
    assert_eq!(initiated.outboxes.len(), 1);
    assert_eq!(
        repository.load(attempt_id).await.unwrap(),
        Some(initiated.clone())
    );

    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x43; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .load(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .target_access_state
            .as_deref(),
        Some(b"joiner-target-access".as_slice())
    );
    assert_eq!(
        prepared.purpose,
        uc_core::membership::AdmissionOutboxPurposeV1::Prepared
    );
}

#[tokio::test]
async fn durable_join_starts_once_and_survives_owner_restart() {
    use uc_core::membership::{
        AdmissionAttemptRepositoryPort, AdmissionOutboxPurposeV1, JoinerAdmissionStageV1,
    };

    let directory = tempfile::tempdir().unwrap();
    let repository: Arc<dyn AdmissionAttemptRepositoryPort> =
        durable_admission_repository(&directory, [0x23; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x31; 32]);
    let join_id = [0x32; 16];

    let first = owner
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    assert_eq!(first.local_join_ordinal, Some(0));
    assert!(matches!(
        first.role_state,
        uc_core::membership::AdmissionAttemptRoleStateV1::Joiner(
            uc_core::membership::JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Initiated
            }
        )
    ));
    assert_eq!(first.outboxes.len(), 1);
    assert_eq!(
        first.outboxes[0].purpose,
        AdmissionOutboxPurposeV1::JoinRequest
    );

    let profile = super::ProfileWorkspaceConvergence::new(
        Arc::clone(&repository),
        DeviceId::new("joiner"),
        Arc::new(UnusedClock),
    );
    let fresh_snapshot = profile.query_device_trust().await.unwrap();
    assert!(fresh_snapshot.revision > 0);
    assert!(fresh_snapshot.devices.is_empty());
    assert!(matches!(
        fresh_snapshot.current_join,
        Some(super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            ..
        }) if projected_join_id == join_id
    ));

    let reopened = durable_admission_owner(Arc::clone(&repository));
    assert!(matches!(
        reopened.current_local_join().await.unwrap(),
        Some(super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            target_space_id: None,
            sponsor_device_id: None,
            sponsor_identity_fingerprint: None,
            cancel_requested: false,
        }) if projected_join_id == join_id
    ));
    let second = reopened
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    assert_eq!(second, first);
    assert!(matches!(
        reopened.cancel_local_join(join_id).await.unwrap(),
        super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            cancel_requested: true,
            ..
        } if projected_join_id == join_id
    ));
    assert!(matches!(
        reopened.cancel_local_join([0xff; 16]).await,
        Err(WorkspaceConvergenceError::JoinNotFound)
    ));
    assert!(matches!(
        durable_admission_owner(repository)
            .current_local_join()
            .await
            .unwrap(),
        Some(super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            cancel_requested: true,
            ..
        }) if projected_join_id == join_id
    ));
}

#[tokio::test]
async fn automatic_recovery_keeps_the_same_join_identity() {
    use super::admission::DurableJoinRecoveryMaterialV1;
    use uc_core::membership::AdmissionAttemptRepositoryPort;

    let directory = tempfile::tempdir().unwrap();
    let repository: Arc<dyn AdmissionAttemptRepositoryPort> =
        durable_admission_repository(&directory, [0x73; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x74; 32]);
    let material = DurableJoinRecoveryMaterialV1 {
        pending_security_state: b"private-join-state".to_vec(),
        candidate_key_package: b"public-key-package".to_vec(),
        member_instance: MemberInstanceId::from_bytes([0x75; 32]),
        resume_public_key: vec![0x76; 32],
        resume_private_key: vec![0x77; 32],
    };

    owner
        .start_join_with_recovery_material(
            attempt_id,
            [0x78; 16],
            b"sponsor",
            b"join-request",
            &material,
        )
        .await
        .unwrap();

    let reopened = durable_admission_owner(repository);
    assert_eq!(
        reopened
            .load_join_recovery_material(attempt_id)
            .await
            .unwrap(),
        material
    );
}

struct RotatingPreparation {
    calls: std::sync::atomic::AtomicUsize,
}

struct FailingPreparation;

struct FailingTargetAccess;

#[async_trait::async_trait]
impl uc_core::ports::space::PrepareAdmissionTargetAccessPort for FailingTargetAccess {
    async fn prepare_target_access(
        &self,
        _target_space_id: &SpaceId,
        _passphrase: &uc_core::crypto::domain::Passphrase,
    ) -> Result<
        uc_core::space_access::PreparedAdmissionTargetAccess,
        uc_core::ports::space::SpaceAccessError,
    > {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "unused".to_owned(),
        ))
    }
}

#[async_trait::async_trait]
impl uc_core::ports::space::GroupAdmissionPort for FailingPreparation {
    async fn prepare_group_join(
        &self,
        _device_id: &DeviceId,
    ) -> Result<uc_core::space_access::PreparedGroupJoin, uc_core::ports::space::SpaceAccessError>
    {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "injected join material failure".to_owned(),
        ))
    }

    async fn admit_group_member(
        &self,
        _space_id: &SpaceId,
        _sponsor_device_id: &DeviceId,
        _joiner_device_id: &DeviceId,
        _existing_member_ids: &[DeviceId],
        _key_package: &[u8],
    ) -> Result<uc_core::space_access::GroupAdmission, uc_core::ports::space::SpaceAccessError>
    {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "unused".to_owned(),
        ))
    }

    async fn install_group_join(
        &self,
        _space_id: &SpaceId,
        _passphrase: &uc_core::crypto::domain::Passphrase,
        _pending: uc_core::space_access::PreparedGroupJoin,
        _welcome: &[u8],
        _encrypted_key_catalog: &[u8],
        _group_epoch: u64,
    ) -> Result<(), uc_core::ports::space::SpaceAccessError> {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "unused".to_owned(),
        ))
    }
}

#[async_trait::async_trait]
impl uc_core::ports::space::GroupAdmissionPort for RotatingPreparation {
    async fn prepare_group_join(
        &self,
        _device_id: &DeviceId,
    ) -> Result<uc_core::space_access::PreparedGroupJoin, uc_core::ports::space::SpaceAccessError>
    {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) as u8 + 1;
        Ok(
            uc_core::space_access::PreparedGroupJoin::new(vec![call], vec![call.wrapping_add(1)])
                .with_member_instance(MemberInstanceId::from_bytes([call; 32])),
        )
    }

    async fn admit_group_member(
        &self,
        _space_id: &SpaceId,
        _sponsor_device_id: &DeviceId,
        _joiner_device_id: &DeviceId,
        _existing_member_ids: &[DeviceId],
        _key_package: &[u8],
    ) -> Result<uc_core::space_access::GroupAdmission, uc_core::ports::space::SpaceAccessError>
    {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "unused".to_owned(),
        ))
    }

    async fn install_group_join(
        &self,
        _space_id: &SpaceId,
        _passphrase: &uc_core::crypto::domain::Passphrase,
        _pending: uc_core::space_access::PreparedGroupJoin,
        _welcome: &[u8],
        _encrypted_key_catalog: &[u8],
        _group_epoch: u64,
    ) -> Result<(), uc_core::ports::space::SpaceAccessError> {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "unused".to_owned(),
        ))
    }
}

fn advance_joiner_to_candidate_or_prepared(
    attempt: &mut uc_core::membership::AdmissionAttemptV1,
    stage: uc_core::membership::JoinerAdmissionStageV1,
) {
    attempt.role_state = uc_core::membership::AdmissionAttemptRoleStateV1::Joiner(
        uc_core::membership::JoinerAdmissionStateV1 { stage },
    );
    attempt.lineage_id = Some("target-space".to_owned());
    attempt.base_history_position = Some(vec![1]);
    attempt.candidate_event = Some(vec![2]);
    attempt.candidate_event_id = Some([3; 32]);
    attempt.target_members_digest = Some([4; 32]);
    attempt.security_commitment = Some(vec![5]);
    attempt.security_commit = Some(vec![6]);
    attempt.security_welcome = Some(vec![7]);
    attempt.target_protection_group_id = Some("target-group".to_owned());
    attempt.target_key_catalog = Some(vec![8]);
    attempt.target_relationships = Some(Vec::new());
    attempt.existing_member_security_deliveries = Some(Vec::new());
    attempt.staged_security_state = Some(vec![9]);
    attempt.base_membership_history = Some(vec![10]);
    if stage == uc_core::membership::JoinerAdmissionStageV1::Prepared {
        attempt.verified_membership_history = Some(vec![11]);
        attempt.prepared_proof = Some(vec![12]);
        attempt.target_access_state = Some(vec![13]);
    }
}

#[tokio::test]
async fn explicit_join_supersedes_initiated_attempt_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x7a; 16]);
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };

    let first = durable_admission_owner(Arc::clone(&repository))
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"sponsor",
            b"join-request",
            false,
        )
        .await
        .unwrap();
    let second = durable_admission_owner(Arc::clone(&repository))
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"different-sponsor",
            b"different-join-request",
            false,
        )
        .await
        .unwrap();

    assert_eq!(preparation.calls.load(Ordering::SeqCst), 2);
    assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
    assert_ne!(first.attempt.join_id, second.attempt.join_id);
    assert_ne!(
        first.attempt.local_join_ordinal,
        second.attempt.local_join_ordinal
    );
    assert!(!first.attempt.preserve_unreadable_history);
    assert_ne!(
        first.prepared_group_join.key_package,
        second.prepared_group_join.key_package
    );
    assert_ne!(
        first.prepared_group_join.private_state(),
        second.prepared_group_join.private_state()
    );
    assert_ne!(
        first.prepared_group_join.member_instance(),
        second.prepared_group_join.member_instance()
    );
    let previous = repository
        .load(first.attempt.attempt_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        previous.terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
    );
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        second.attempt.attempt_id
    );

    let confirmed_directory = tempfile::tempdir().unwrap();
    let confirmed_repository = durable_admission_repository(&confirmed_directory, [0x7b; 16]);
    let confirmed = durable_admission_owner(confirmed_repository)
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"other-sponsor",
            b"other-join-request",
            true,
        )
        .await
        .unwrap();
    assert!(confirmed.attempt.preserve_unreadable_history);
}

#[tokio::test]
async fn explicit_join_with_same_invitation_starts_new_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe0; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"same-sponsor",
            b"same-request",
            false,
        )
        .await
        .unwrap();
    let second = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"same-sponsor",
            b"same-request",
            false,
        )
        .await
        .unwrap();

    assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
    assert_ne!(first.attempt.join_id, second.attempt.join_id);
    assert_eq!(preparation.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn explicit_join_material_failure_keeps_previous_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xf3; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"first-request",
            false,
        )
        .await
        .unwrap();
    let metadata_before = repository.profile_metadata().await.unwrap();

    assert!(owner
        .prepare_join_before_network(
            &FailingPreparation,
            &DeviceId::new("joiner"),
            b"second-sponsor",
            b"second-request",
            false,
        )
        .await
        .is_err());

    assert_eq!(
        repository.profile_metadata().await.unwrap(),
        metadata_before
    );
    assert_eq!(
        repository.load(first.attempt.attempt_id).await.unwrap(),
        Some(first.attempt.clone())
    );
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        first.attempt.attempt_id
    );
}

#[tokio::test]
async fn explicit_join_supersedes_candidate_before_prepared() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe1; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"first-request",
            false,
        )
        .await
        .unwrap();
    let mut candidate = first.attempt.clone();
    candidate.record_version = 1;
    advance_joiner_to_candidate_or_prepared(
        &mut candidate,
        uc_core::membership::JoinerAdmissionStageV1::Candidate,
    );
    repository
        .compare_and_advance(first.attempt.attempt_id, 0, &candidate)
        .await
        .unwrap();

    let second = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"second-sponsor",
            b"second-request",
            false,
        )
        .await
        .unwrap();

    assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
    assert_eq!(
        repository
            .load(first.attempt.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
    );
}

#[tokio::test]
async fn explicit_join_after_prepared_returns_stable_conflict() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe2; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"first-request",
            false,
        )
        .await
        .unwrap();
    let mut prepared = first.attempt.clone();
    prepared.record_version = 1;
    advance_joiner_to_candidate_or_prepared(
        &mut prepared,
        uc_core::membership::JoinerAdmissionStageV1::Prepared,
    );
    repository
        .compare_and_advance(first.attempt.attempt_id, 0, &prepared)
        .await
        .unwrap();
    let before = repository.profile_metadata().await.unwrap();

    assert!(matches!(
        owner.preflight_join_source(false).await,
        Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded)
    ));

    assert!(matches!(
        owner
            .prepare_join_before_network(
                &preparation,
                &DeviceId::new("joiner"),
                b"second-sponsor",
                b"second-request",
                false,
            )
            .await,
        Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded)
    ));
    assert_eq!(preparation.calls.load(Ordering::SeqCst), 1);
    assert_eq!(repository.profile_metadata().await.unwrap(), before);
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        first.attempt.attempt_id
    );
}

#[tokio::test]
async fn concurrent_explicit_joins_leave_one_current_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe3; 16]);
    let first_owner = durable_admission_owner(Arc::clone(&repository));
    let second_owner = durable_admission_owner(Arc::clone(&repository));
    let first_preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let second_preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first_device = DeviceId::new("joiner");
    let second_device = DeviceId::new("joiner");

    let (first, second) = tokio::join!(
        first_owner.prepare_join_before_network(
            &first_preparation,
            &first_device,
            b"first-sponsor",
            b"first-request",
            false,
        ),
        second_owner.prepare_join_before_network(
            &second_preparation,
            &second_device,
            b"second-sponsor",
            b"second-request",
            false,
        )
    );
    let first = first.unwrap();
    let second = second.unwrap();
    let current = repository
        .project_current_local_join()
        .await
        .unwrap()
        .unwrap();

    assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
    assert!(
        current.attempt_id == first.attempt.attempt_id
            || current.attempt_id == second.attempt.attempt_id
    );
    let historical_id = if current.attempt_id == first.attempt.attempt_id {
        second.attempt.attempt_id
    } else {
        first.attempt.attempt_id
    };
    assert_eq!(
        repository
            .load(historical_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
    );
    assert_eq!(
        repository
            .profile_metadata()
            .await
            .unwrap()
            .next_local_join_ordinal,
        2
    );
}

#[tokio::test]
async fn inbound_admission_blocks_explicit_join_without_retry_loop() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe4; 16]);
    let mut inbound = uc_core::membership::AdmissionAttemptV1::new_joiner(
        uc_core::membership::AdmissionAttemptId::from_bytes([0xe5; 32]),
        [0xe6; 16],
        uc_core::membership::JoinerAdmissionStageV1::Initiated,
    );
    inbound.join_id = None;
    inbound.role_state = uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(
        uc_core::membership::SponsorAdmissionStateV1 {
            stage: uc_core::membership::SponsorAdmissionStageV1::Accepted,
        },
    );
    inbound.invitation_claim = Some(vec![1]);
    repository.create(&inbound, None, None).await.unwrap();
    let owner = durable_admission_owner(repository);
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        owner.prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"sponsor",
            b"request",
            false,
        ),
    )
    .await
    .expect("explicit join must not retry indefinitely");

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::AdmissionInProgress)
    ));
}

#[tokio::test]
async fn admission_unavailable_keeps_the_exact_pending_join() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x24; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x25; 32]);
    let started = owner
        .start_join(
            attempt_id,
            [0x26; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let metadata_before = repository.profile_metadata().await.unwrap();

    let retry = owner
        .record_admission_unavailable(attempt_id, &started.outboxes[0])
        .await
        .unwrap();

    assert_eq!(retry, started.outboxes[0]);
    assert_eq!(repository.load(attempt_id).await.unwrap(), Some(started));
    assert_eq!(
        repository.profile_metadata().await.unwrap(),
        metadata_before
    );
}

#[tokio::test]
async fn delivery_ack_clears_only_the_exact_supported_outbox() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x27; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x28; 32]);
    let started = owner
        .start_join(
            attempt_id,
            [0x29; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let mut wrong = super::admission::admission_acknowledgment(&started.outboxes[0]);
    wrong.payload_digest[0] ^= 0xff;

    assert!(owner
        .acknowledge_delivery(attempt_id, &wrong)
        .await
        .is_err());
    assert!(!repository.load(attempt_id).await.unwrap().unwrap().outboxes[0].superseded);

    let exact = super::admission::admission_acknowledgment(&started.outboxes[0]);
    owner
        .acknowledge_delivery(attempt_id, &exact)
        .await
        .unwrap();
    let saved = repository.load(attempt_id).await.unwrap().unwrap();
    assert!(saved.outboxes[0].superseded);
    assert!(saved.inbox_dedup.contains(&exact));
}

#[tokio::test]
async fn first_sponsor_admission_records_the_initial_member_instance() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let session = uc_core::ports::pairing::PairingSessionId::new("first-admission");
    harness
        .owner
        .begin_admission(&session, &DeviceId::new("device-b"), 0)
        .await
        .unwrap();

    harness
        .owner
        .commit_joiner_admission(
            &session,
            admission_facts_for(b, &DeviceId::new("device-b")),
            vec![5],
        )
        .await
        .expect("a newly created space can sponsor its first admission");

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.own_instance, Some(a));
    assert_eq!(state.effective_members(), [a, b].into());
}

#[tokio::test]
async fn single_member_legacy_history_forms_an_honest_v2_admission_base() {
    let repository = MemoryWorkspaceRepository::default();
    let device_id = DeviceId::new("device-a");
    let credential = uc_core::membership::MembershipCredential::new(1, vec![0x71; 32]);
    let mut deps = test_deps(Arc::new(repository), device_id.as_str(), Vec::new());
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: device_id.clone(),
        credential: credential.clone(),
    });
    let owner = WorkspaceConvergence::new(deps);
    owner.initialize_upgraded_legacy_space().await.unwrap();

    let history = owner.verified_admission_base_history().await.unwrap();

    let own_instance = credential.member_instance_id(&device_id);
    assert_eq!(history.effective_members(), [own_instance].into());
    assert_eq!(history.active_members(), [own_instance].into());
    assert_eq!(history.credential_for(own_instance), Some(&credential));
}

#[tokio::test]
async fn multi_member_legacy_history_cannot_claim_complete_v2_verification() {
    let repository = MemoryWorkspaceRepository::default();
    let device_id = DeviceId::new("device-a");
    let credential = uc_core::membership::MembershipCredential::new(1, vec![0x72; 32]);
    let mut deps = test_deps(Arc::new(repository), device_id.as_str(), Vec::new());
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: device_id.clone(),
        credential,
    });
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    let owner = WorkspaceConvergence::new(deps);
    owner.initialize_upgraded_legacy_space().await.unwrap();

    assert!(matches!(
        owner.verified_admission_base_history().await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
}

#[tokio::test]
async fn sponsor_candidate_uses_only_members_active_in_verified_history() {
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionAttemptRoleStateV1, AdmissionAttemptV1,
        AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, JoinerAdmissionStateV1, MembershipCredential, MembershipEventV2,
        MembershipOperationV2, ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
    };
    use uc_core::pairing::{InvitationCode, JoinerRequest, PairingSecurityCapability};

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x41; 16]);
    let seed_attempt_id = AdmissionAttemptId::from_bytes([0x42; 32]);
    let (mut history, added_member, _) =
        admission_verification_fixture_for_lineage(*seed_attempt_id.as_bytes(), SPACE);
    history
        .verify_and_receive_event(added_member.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let sponsor_device = DeviceId::new("sponsor");
    let sponsor_member = sponsor_credential.member_instance_id(&sponsor_device);
    let MembershipOperationV2::AddDevice {
        admission: removed_admission,
    } = &added_member.operation
    else {
        unreachable!()
    };
    let removal_operation = MembershipOperationV2::RemoveDevice {
        member: removed_admission.facts.member_instance,
    };
    let removal_members_digest = history
        .expected_resulting_members_digest(Some(added_member.event_id()), &removal_operation)
        .unwrap();
    let mut removal = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        SPACE.to_owned(),
        Some(added_member.event_id()),
        9,
        [0x43; 16],
        sponsor_member,
        sponsor_credential.credential_id,
        sponsor_credential.signature_algorithm_version,
        removal_operation,
        removal_members_digest,
        [0x44; 32],
        Vec::new(),
        None,
        Vec::new(),
    );
    removal.signature =
        DeterministicHistoricalVerifier.sign(&sponsor_credential, &removal.signing_payload());
    history
        .verify_and_receive_event(removal, &DeterministicHistoricalVerifier)
        .unwrap();
    assert_eq!(history.active_members(), [sponsor_member].into());

    let mut seed = AdmissionAttemptV1::new_joiner(
        seed_attempt_id,
        [0x45; 16],
        JoinerAdmissionStageV1::Rejected,
    );
    seed.local_join_ordinal = Some(0);
    seed.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
        stage: JoinerAdmissionStageV1::Rejected,
    });
    seed.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
    seed.rejection_reason = Some(AdmissionRejectionReasonV1::Cancelled);
    admission_repository
        .create(&seed, None, Some(&history.encode_persisted_v2().unwrap()))
        .await
        .unwrap();

    let workspace_repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(sponsor_member);
    workspace_repository.save_state(&state).await.unwrap();
    let security = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut deps = test_deps(
        Arc::new(workspace_repository),
        sponsor_device.as_str(),
        Vec::new(),
    );
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: sponsor_device.clone(),
        credential: sponsor_credential,
    });
    deps.membership_identity = Arc::new(FixedMembershipIdentity {
        space: SpaceId::from_str(SPACE),
        device_id: sponsor_device.clone(),
    });
    deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: sponsor_device,
    });
    deps.prepare_sponsor_admission_security = security.clone();
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("joiner"),
        legacy_member("stale-removed-member"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let attempt_id = AdmissionAttemptId::from_bytes([0x46; 32]);
    let invitation = InvitationCode::new("candidate-history-filter");
    let joiner_device = DeviceId::new("new-device");
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x47; 32]);
    let joiner_member = joiner_credential.member_instance_id(&joiner_device);
    let mut facts = admission_facts_for(joiner_member, &joiner_device);
    facts.identity_signature =
        DeterministicHistoricalVerifier.sign(&joiner_credential, &facts.signing_payload());
    let binding = crate::space::admission::adapter::stable_join_request_binding(
        &joiner_device,
        &facts.identity_fingerprint,
    );
    let request_message = super::admission::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::JoinRequest,
        invitation.as_str().as_bytes(),
        None,
        &binding,
    );
    let request = JoinerRequest {
        attempt_id: *attempt_id.as_bytes(),
        join_id: [0x48; 16],
        request_message_id: request_message.message_id,
        invitation_code: invitation,
        device_id: joiner_device,
        device_name: facts.device_name.clone(),
        identity_fingerprint: facts.identity_fingerprint.clone(),
        nonce: Vec::new(),
        transport_address_blob: facts.transport_address_blob.clone(),
        security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
        key_package: b"candidate-key-package".to_vec(),
        member_instance: joiner_member,
        membership_credential: joiner_credential,
        resume_public_key: vec![0x49; 32],
        admission: facts,
    };

    let frame = owner.prepare_sponsor_candidate(&request).await.unwrap();

    assert_eq!(
        frame.kind,
        uc_core::pairing::DurableAdmissionMessageKind::Candidate
    );
    let requests = security.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].existing_recipients.is_empty());
    let payload =
        super::admission::DurableAdmissionCandidatePayloadV1::decode(&frame.payload).unwrap();
    assert_eq!(payload.candidate.target_relationships.len(), 2);
    assert!(payload
        .candidate
        .target_relationships
        .iter()
        .all(|facts| facts.device_id.as_str() != "joiner"
            && facts.device_id.as_str() != "stale-removed-member"));
    assert_eq!(payload.candidate.resume_public_key, vec![0x49; 32]);
}

#[tokio::test]
async fn sponsor_rejects_admission_when_persisted_and_current_local_instances_differ() {
    let old_a = instance(0x0b);
    let current_a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    let genesis = membership_event(None, 0, old_a, old_a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), old_a);
    history.receive_verified(genesis).unwrap();
    state.own_instance = Some(old_a);
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    let deps = test_deps(
        Arc::new(repository.clone()),
        "device-a",
        vec![(DeviceId::new("device-a"), current_a)],
    );
    let owner = WorkspaceConvergence::new(deps);
    let session = uc_core::ports::pairing::PairingSessionId::new("stale-local-instance");

    let result = owner
        .begin_admission(&session, &DeviceId::new("device-c"), 1)
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(message))
            if message == "current member identity does not match persisted membership history"
    ));
    assert!(repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_admissions
        .is_empty());
}

#[tokio::test]
async fn admission_recovery_starts_with_legacy_migration_import() {
    let directory = tempfile::tempdir().unwrap();
    let recovery = Arc::new(RecordingLegacyMigrationRecovery::default());
    let mut deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "device-1",
        Vec::new(),
    );
    deps.admission_attempts = durable_admission_repository(&directory, [0x71; 16]);
    deps.legacy_migration_recovery = recovery.clone();
    let owner = WorkspaceConvergence::new(deps);

    owner.recover_pending_admissions().await.unwrap();

    assert_eq!(recovery.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sponsor_recovery_finishes_the_same_activation_after_completion_save_fails() {
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionOutboxPurposeV1, MembershipCredential,
        SponsorAdmissionStageV1, SponsorAdmissionStateV1, VersionedMembershipHistory,
        ED25519_SIGNATURE_ALGORITHM_V1,
    };

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x72; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x73; 16]);
    let sponsor_transaction = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner_transaction = durable_admission_owner(joiner_repository);
    let attempt_id = AdmissionAttemptId::from_bytes([0x74; 32]);
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let initiated = joiner_transaction
        .start_join(
            attempt_id,
            [0x75; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let candidate_message = sponsor_transaction
        .sponsor_accept_and_offer(
            attempt_id,
            [0x76; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    sponsor_transaction
        .record_invitation_consume_result(
            attempt_id,
            super::admission::InvitationConsumeResultV1::Consumed,
        )
        .await
        .unwrap();
    let prepared = joiner_transaction
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let commit = sponsor_transaction
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let receipt_payload = postcard::to_stdvec(&activation_receipt).unwrap();
    let applied = joiner_transaction
        .joiner_apply(
            attempt_id,
            &commit,
            &activation_receipt,
            b"sponsor",
            &receipt_payload,
        )
        .await
        .unwrap();
    let applied_frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Applied,
        message_id: applied.message_id,
        predecessor_message_id: applied.predecessor_message_id,
        payload: receipt_payload.clone(),
    };
    let committed_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let racing_repository = Arc::new(HistoryRaceAdmissionRepository {
        inner: Arc::clone(&sponsor_repository),
        inject_once: AtomicBool::new(true),
        replacement_history: committed_history,
    });
    let first_activation = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let member_repo = Arc::new(RecordingMemberRepo::default());
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let mut first_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "sponsor",
        Vec::new(),
    );
    first_deps.admission_attempts = racing_repository;
    first_deps.activate_sponsor_admission_security = first_activation.clone();
    first_deps.member_repo = member_repo.clone();
    first_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential.clone(),
    });
    let first_owner = WorkspaceConvergence::new(first_deps);

    assert!(first_owner
        .complete_sponsor_applied(&applied_frame)
        .await
        .is_err());
    assert_eq!(
        first_activation.activation_requests.lock().unwrap().len(),
        1
    );
    let interrupted = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(interrupted.write_ahead_recovery.is_some());
    assert!(interrupted.completion.is_none());
    assert!(member_repo
        .get(&DeviceId::new("joiner"))
        .await
        .unwrap()
        .is_some());
    assert!(matches!(
        interrupted.role_state,
        uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Committed
        })
    ));

    let resumed_activation = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut resumed_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "sponsor",
        Vec::new(),
    );
    resumed_deps.admission_attempts = Arc::clone(&sponsor_repository);
    resumed_deps.activate_sponsor_admission_security = resumed_activation.clone();
    resumed_deps.member_repo = member_repo.clone();
    resumed_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    let resumed_owner = WorkspaceConvergence::new(resumed_deps);

    assert_eq!(resumed_owner.recover_pending_admissions().await.unwrap(), 1);
    let recovered = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(recovered.write_ahead_recovery.is_none());
    assert!(recovered.completion.is_some());
    assert_eq!(
        recovered
            .outboxes
            .iter()
            .filter(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
            })
            .count(),
        1
    );
    let recovered_history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(recovered_history.active_members().len(), 2);
    assert_eq!(
        resumed_activation.activation_requests.lock().unwrap().len(),
        1
    );

    resumed_owner.recover_pending_admissions().await.unwrap();
    assert_eq!(
        resumed_activation.activation_requests.lock().unwrap().len(),
        1
    );
    assert_eq!(
        sponsor_repository
            .load(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .outboxes
            .iter()
            .filter(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn committed_admission_records_the_effective_members_in_signed_history() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let session = uc_core::ports::pairing::PairingSessionId::new("history-admission");
    harness.owner.record_local_readiness(a).await.unwrap();
    harness
        .owner
        .begin_admission(&session, &DeviceId::new("device-b"), 0)
        .await
        .unwrap();
    let joiner = uc_core::membership::AdmissionChangeFacts {
        member_instance: b,
        device_id: DeviceId::new("device-b"),
        device_name: "b".to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        transport_public_key: vec![2; 32],
        transport_address_blob: vec![3],
        identity_signature: vec![4],
    };

    harness
        .owner
        .commit_joiner_admission(&session, joiner, vec![5])
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    let history = state.membership_reconciliation.as_ref().unwrap();
    assert_eq!(history.effective_members(), [a, b].into());
    assert_eq!(
        history.device_for_member(&a),
        Some(DeviceId::new("device-a"))
    );
    assert_eq!(
        history.device_for_member(&b),
        Some(DeviceId::new("device-b"))
    );
    assert_eq!(state.effective_members(), [a, b].into());

    harness
        .owner
        .submit_removal(&DeviceId::new("device-b"))
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    let history = state.membership_reconciliation.as_ref().unwrap();
    assert_eq!(history.effective_members(), [a].into());
    assert_eq!(state.effective_members(), [a].into());
}

#[tokio::test]
async fn sponsor_rejects_a_joiner_with_an_active_member_instance() {
    let c = instance(0x0c);
    let a = instance(0x0a);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-c"), c),
            (DeviceId::new("device-a"), a),
        ],
    );
    let genesis = membership_event(None, 0, c, c, "device-c", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, c, a, "device-a", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness
            .owner
            .admission_decision_for_joiner(2, &DeviceId::new("device-a"))
            .await,
        MembershipAdmissionDecision::Unavailable
    );
}

#[tokio::test]
async fn sponsor_allows_a_removed_device_to_rejoin_with_a_new_instance() {
    let c = instance(0x0c);
    let a = instance(0x0a);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-c"), c),
            (DeviceId::new("device-a"), a),
        ],
    );
    let genesis = membership_event(None, 0, c, c, "device-c", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, c, a, "device-a", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        c,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    history.receive_verified(removal).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness
            .owner
            .admission_decision_for_joiner(3, &DeviceId::new("device-a"))
            .await,
        MembershipAdmissionDecision::Allowed
    );
}
