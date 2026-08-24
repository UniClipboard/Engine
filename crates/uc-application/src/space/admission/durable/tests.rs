use crate::space::workspace_membership::tests::*;

async fn seed_initiated_join(
    repository: &dyn TestAdmissionRepository,
    attempt_id: uc_core::membership::AdmissionAttemptId,
    join_id: [u8; 16],
) -> uc_core::membership::AdmissionAttemptV1 {
    use uc_core::membership::{
        AdmissionAttemptV1, AdmissionOutboxPurposeV1, JoinerAdmissionStageV1,
    };

    let metadata = repository.profile_metadata().await.unwrap();
    let mut attempt =
        AdmissionAttemptV1::new_joiner(attempt_id, join_id, JoinerAdmissionStageV1::Initiated);
    attempt.local_join_ordinal = Some(metadata.next_local_join_ordinal);
    attempt.joiner_pending_security_state = Some(b"joiner-pending-state".to_vec());
    attempt.candidate_key_package = Some(b"joiner-key-package".to_vec());
    attempt.target_access_state = Some(b"joiner-target-access".to_vec());
    attempt
        .outboxes
        .push(super::admission::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            b"sponsor",
            None,
            b"join-request",
        ));
    repository.create(&attempt, None, None).await.unwrap();
    attempt
}

async fn seed_superseded_and_current_join(
    repository: &Arc<dyn TestAdmissionRepository>,
) -> (
    uc_core::membership::AdmissionAttemptId,
    uc_core::membership::AdmissionAttemptId,
) {
    use crate::deps::LocalJoinStartMutationV1;
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionAttemptV1, AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1,
        JoinerAdmissionStageV1, MemberInstanceId,
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

#[derive(Default)]
struct RecordingAdmissionDelivery {
    routes: std::sync::Mutex<
        Vec<(
            uc_core::membership::AdmissionAttemptId,
            Option<crate::deps::AdmissionOutboxDeliveryRouteV1>,
        )>,
    >,
}

#[async_trait]
impl crate::deps::AdmissionOutboxDeliveryPort for RecordingAdmissionDelivery {
    async fn deliver(
        &self,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        message: &uc_core::membership::AdmissionOutboxMessageV1,
        route: Option<&crate::deps::AdmissionOutboxDeliveryRouteV1>,
    ) -> Result<
        crate::deps::AdmissionOutboxDeliveryResultV1,
        crate::deps::AdmissionOutboxDeliveryError,
    > {
        self.routes
            .lock()
            .unwrap()
            .push((attempt_id, route.cloned()));
        if message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::CancelRequested {
            return Ok(crate::deps::AdmissionOutboxDeliveryResultV1::Rejected(
                super::admission::durable_admission_message(
                    attempt_id,
                    uc_core::membership::AdmissionOutboxPurposeV1::Rejected,
                    &message.recipient,
                    Some(message.message_id),
                    &postcard::to_stdvec(&(
                        uc_core::membership::AdmissionRejectionReasonV1::Cancelled,
                        b"cancelled".to_vec(),
                    ))
                    .unwrap(),
                ),
            ));
        }
        Ok(crate::deps::AdmissionOutboxDeliveryResultV1::Persisted(
            super::admission::admission_acknowledgment(message),
        ))
    }
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
    deps.membership_history_repo = Arc::clone(&repository);
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceMembership::new(deps);
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
    deps.membership_history_repo = Arc::clone(&repository);
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceMembership::new(deps);
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
    deps.membership_history_repo = Arc::clone(&repository);
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceMembership::new(deps);
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
async fn compacted_superseded_join_rejects_late_protocol_messages() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xf7; 16]);
    let transaction = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let previous = repository.load(previous_id).await.unwrap().unwrap();
    let cleanup = previous
        .outboxes
        .iter()
        .find(|message| {
            message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::CancelRequested
                && !message.superseded
        })
        .unwrap();
    crate::space::admission::cancel_space_join::confirm_superseded_join_cleanup_delivery(
        repository.as_ref(),
        previous_id,
        &super::admission::admission_acknowledgment(cleanup),
    )
    .await
    .unwrap();
    transaction.compact_if_settled(previous_id).await.unwrap();

    let mut deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "device-1",
        Vec::new(),
    );
    deps.membership_history_repo = Arc::clone(&repository);
    deps.admission_attempts = Arc::clone(&repository);
    let owner = WorkspaceMembership::new(deps);
    let candidate = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Candidate,
        b"device-1",
        Some([5; 32]),
        b"late-candidate",
    );
    let candidate_frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *previous_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Candidate,
        message_id: candidate.message_id,
        predecessor_message_id: candidate.predecessor_message_id,
        payload: candidate.payload,
    };
    assert!(matches!(
        owner
            .prepare_joiner_candidate(
                &candidate_frame,
                &FailingPreparation,
                &FailingTargetAccess,
                &uc_core::crypto::domain::Passphrase::new("passphrase"),
            )
            .await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));

    let commit = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Commit,
        b"device-1",
        Some([0xf8; 32]),
        b"late-commit",
    );
    let commit_frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *previous_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Commit,
        message_id: commit.message_id,
        predecessor_message_id: commit.predecessor_message_id,
        payload: commit.payload,
    };
    assert!(matches!(
        owner
            .apply_joiner_commit(&commit_frame, &FailingPreparation)
            .await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));

    let complete = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Complete,
        b"device-1",
        Some([0xf9; 32]),
        b"late-complete",
    );
    let complete_frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *previous_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Complete,
        message_id: complete.message_id,
        predecessor_message_id: complete.predecessor_message_id,
        payload: complete.payload,
    };
    assert!(matches!(
        owner.activate_joiner_complete(&complete_frame).await,
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
async fn superseded_rejection_only_confirms_old_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xdb; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let rejected = super::admission::durable_admission_message(
        previous_id,
        uc_core::membership::AdmissionOutboxPurposeV1::Rejected,
        b"sponsor",
        Some([0xd3; 32]),
        &postcard::to_stdvec(&(
            uc_core::membership::AdmissionRejectionReasonV1::Cancelled,
            b"cleanup-confirmed".to_vec(),
        ))
        .unwrap(),
    );

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
async fn superseded_delivery_acknowledgment_only_settles_old_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xf6; 16]);
    let (previous_id, current_id) = seed_superseded_and_current_join(&repository).await;
    let previous = repository.load(previous_id).await.unwrap().unwrap();
    let cleanup = previous
        .outboxes
        .iter()
        .find(|message| {
            message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::CancelRequested
                && !message.superseded
        })
        .unwrap();
    let acknowledgment = super::admission::admission_acknowledgment(cleanup);

    crate::space::admission::cancel_space_join::confirm_superseded_join_cleanup_delivery(
        repository.as_ref(),
        previous_id,
        &acknowledgment,
    )
    .await
    .unwrap();

    let settled_previous = repository.load(previous_id).await.unwrap().unwrap();
    assert!(settled_previous
        .outboxes
        .iter()
        .all(|message| message.superseded));
    assert!(settled_previous.inbox_dedup.contains(&acknowledgment));
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
            b"first-sponsor-address",
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
            b"second-sponsor-address",
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
            b"third-sponsor-address",
            b"third-request",
            false,
        )
        .await
        .unwrap();

    let delivery = RecordingAdmissionDelivery::default();
    let report = owner.recover_with(&delivery).await.unwrap();

    assert_eq!(report.deliveries_attempted, 3);
    assert_eq!(report.deliveries_confirmed, 3);
    assert_eq!(report.attempts_compacted, 2);
    let routes = delivery.routes.lock().unwrap();
    assert_eq!(routes.len(), 3);
    assert!(routes.contains(&(
        first.attempt.attempt_id,
        Some(crate::deps::AdmissionOutboxDeliveryRouteV1::Continuation(
            b"first-sponsor-address".to_vec(),
        ),),
    )));
    assert!(routes.contains(&(
        second.attempt.attempt_id,
        Some(crate::deps::AdmissionOutboxDeliveryRouteV1::Continuation(
            b"second-sponsor-address".to_vec(),
        ),),
    )));
    assert!(routes.contains(&(current.attempt.attempt_id, None)));
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
        .compare_and_replace_membership_history(None, &completed_history_bytes)
        .await
        .unwrap();

    let mut joiner_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "joiner",
        Vec::new(),
    );
    joiner_deps.membership_history_repo = Arc::clone(&joiner_repository);
    joiner_deps.admission_attempts = Arc::clone(&joiner_repository);
    joiner_deps.historical_membership_signatures = Arc::new(DeterministicHistoricalVerifier);
    let joiner = WorkspaceMembership::new(joiner_deps);

    let mut blocked_helper_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "helper",
        Vec::new(),
    );
    blocked_helper_deps.membership_history_repo = Arc::clone(&helper_repository);
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
    let blocked_helper = WorkspaceMembership::new(blocked_helper_deps);

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
    resumed_helper_deps.membership_history_repo = Arc::clone(&helper_repository);
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
    let resumed_helper = WorkspaceMembership::new(resumed_helper_deps);
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
async fn durable_admission_cancel_and_commit_have_exactly_one_winner() {
    use uc_core::membership::AdmissionOutboxPurposeV1;
    async fn prepared_pair(
        sponsor: &super::DurableAdmissionTransaction,
        joiner_repository: &dyn TestAdmissionRepository,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        join_id: [u8; 16],
    ) -> uc_core::membership::AdmissionOutboxMessageV1 {
        let initiated = seed_initiated_join(joiner_repository, attempt_id, join_id).await;
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
        crate::space::admission::joiner::record_invitation_consume_result(
            sponsor_repository.as_ref(),
            attempt_id,
            crate::space::admission::joiner::InvitationConsumeResultV1::NotFound,
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
    let prepared =
        prepared_pair(&sponsor, joiner_repository.as_ref(), attempt_id, [0x57; 16]).await;
    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    let replayed_rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(replayed_rejected, rejected);
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
    crate::space::admission::sponsor::confirm_rejected_delivery(
        sponsor_repository.as_ref(),
        attempt_id,
        &rejected_ack,
    )
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
    let settled_replay = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(settled_replay, rejected);
    joiner.compact_if_settled(attempt_id).await.unwrap();
    sponsor.compact_if_settled(attempt_id).await.unwrap();
    let compacted_replay = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(compacted_replay, rejected);

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x58; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x59; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x5a; 32]);
    let prepared =
        prepared_pair(&sponsor, joiner_repository.as_ref(), attempt_id, [0x5b; 16]).await;
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
    let joiner_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort> =
        Arc::clone(&joiner_repository);
    let query_pending_transition =
        crate::space::admission::query_pending_space_transition::QueryPendingSpaceTransitionUseCase::new(
            Arc::clone(&joiner_attempts),
        );
    let complete_pending_transition =
        crate::space::admission::complete_pending_space_transition::CompletePendingSpaceTransitionUseCase::new(
            joiner_attempts,
            transition.clone(),
        );
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc7; 32]);
    let initiated = seed_initiated_join(joiner_repository.as_ref(), attempt_id, [0xc8; 16]).await;
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
    assert!(query_pending_transition.execute().await.unwrap());
    let interrupted = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    let expected_active_history = interrupted
        .verified_membership_history
        .clone()
        .expect("activated join must retain its verified history");
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
        assert!(complete_pending_transition.execute().await.is_err());
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

    assert!(matches!(
        complete_pending_transition.execute().await.unwrap(),
        crate::space::admission::CurrentJoinStatus::Active { .. }
    ));
    assert!(!query_pending_transition.execute().await.unwrap());
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
    assert_eq!(
        joiner_repository.load_membership_history().await.unwrap(),
        Some(expected_active_history)
    );
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
    let profile = crate::facade::SpaceJoinFacade::new(Arc::clone(&joiner_repository));
    let pending_ack = profile.recover_completion().await.unwrap().unwrap();
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
    let initiated = seed_initiated_join(joiner_repository.as_ref(), attempt_id, [0xd4; 16]).await;
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
    use crate::space::admission::joiner::InvitationConsumeResultV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x2e; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x2f; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x30; 32]);
    let initiated = seed_initiated_join(joiner_repository.as_ref(), attempt_id, [0x31; 16]).await;
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
    crate::space::admission::joiner::record_invitation_consume_result(
        sponsor_repository.as_ref(),
        attempt_id,
        InvitationConsumeResultV1::Retryable,
    )
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
    crate::space::admission::sponsor::confirm_rejected_delivery(
        sponsor_repository.as_ref(),
        attempt_id,
        &rejected_ack,
    )
    .await
    .unwrap();

    assert!(sponsor.compact_if_settled(attempt_id).await.is_err());
    crate::space::admission::joiner::record_invitation_consume_result(
        sponsor_repository.as_ref(),
        attempt_id,
        InvitationConsumeResultV1::Conflict,
    )
    .await
    .unwrap();
    let after_conflict = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let metadata_after_conflict = sponsor_repository.profile_metadata().await.unwrap();
    crate::space::admission::joiner::record_invitation_consume_result(
        sponsor_repository.as_ref(),
        attempt_id,
        InvitationConsumeResultV1::Conflict,
    )
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
    seed_initiated_join(joiner_repository.as_ref(), joiner_attempt, [0x7b; 16]).await;

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
    let sponsor_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x7e; 32]);
    let initiated =
        seed_initiated_join(remote_repository.as_ref(), sponsor_attempt, [0x7f; 16]).await;
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
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x35; 32]);
    let initiated = seed_initiated_join(joiner_repository.as_ref(), attempt_id, [0x36; 16]).await;
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
        .load_membership_history()
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
    let initiated = seed_initiated_join(joiner_repository.as_ref(), attempt_id, [0x3c; 16]).await;
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
    let before_history = joiner_repository.load_membership_history().await.unwrap();
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
        joiner_repository.load_membership_history().await.unwrap(),
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
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };

    let initiated = joiner
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"invitation-code",
            b"join-request",
            false,
        )
        .await
        .unwrap();
    let attempt_id = initiated.attempt.attempt_id;

    assert!(initiated.attempt.target_access_state.is_none());
    assert_eq!(initiated.attempt.outboxes.len(), 1);
    assert_eq!(
        repository.load(attempt_id).await.unwrap(),
        Some(initiated.attempt.clone())
    );

    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x43; 32],
            &initiated.attempt.outboxes[0],
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
async fn automatic_recovery_keeps_the_same_join_identity() {
    use crate::deps::AdmissionAttemptRepositoryPort;

    let directory = tempfile::tempdir().unwrap();
    let repository: Arc<dyn AdmissionAttemptRepositoryPort> =
        durable_admission_repository(&directory, [0x73; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let started = owner
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"sponsor",
            b"join-request",
            false,
        )
        .await
        .unwrap();
    let material = owner
        .load_join_recovery_material(started.attempt.attempt_id)
        .await
        .unwrap();

    let reopened = durable_admission_owner(repository);
    assert_eq!(
        reopened
            .load_join_recovery_material(started.attempt.attempt_id)
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
impl crate::deps::GroupAdmissionPort for FailingPreparation {
    async fn prepare_group_join(
        &self,
        _device_id: &DeviceId,
    ) -> Result<uc_core::space_access::PreparedGroupJoin, uc_core::ports::space::SpaceAccessError>
    {
        Err(uc_core::ports::space::SpaceAccessError::Internal(
            "injected join material failure".to_owned(),
        ))
    }
}

#[async_trait::async_trait]
impl crate::deps::GroupAdmissionPort for RotatingPreparation {
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
    attempt.identity_binding = Some(vec![14]);
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
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"sponsor",
            b"join-request",
            false,
        )
        .await
        .unwrap();
    let second = durable_admission_owner(Arc::clone(&repository))
        .prepare_join_before_network_without_route(
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
        .prepare_join_before_network_without_route(
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
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"same-sponsor",
            b"same-request",
            false,
        )
        .await
        .unwrap();
    let second = owner
        .prepare_join_before_network_without_route(
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
    assert_ne!(
        first.attempt.local_join_ordinal,
        second.attempt.local_join_ordinal
    );
    assert_ne!(
        first.attempt.resume_public_key,
        second.attempt.resume_public_key
    );
    assert_ne!(
        first.attempt.resume_private_key,
        second.attempt.resume_private_key
    );
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
    assert_eq!(preparation.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn explicit_join_supersedes_initiated_attempt_after_request_delivery_ack() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xe1; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"first-request",
            false,
        )
        .await
        .unwrap();
    let first_request = first.attempt.outboxes[0].clone();
    crate::space::admission::recover_pending_admissions::record_protocol_message_delivered(
        repository.as_ref(),
        first.attempt.attempt_id,
        &super::admission::admission_acknowledgment(&first_request),
    )
    .await
    .unwrap();

    let second = owner
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"second-sponsor",
            b"second-request",
            false,
        )
        .await
        .unwrap();

    assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
    let previous = repository
        .load(first.attempt.attempt_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        previous.terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
    );
    let cleanup = previous
        .outboxes
        .iter()
        .find(|message| {
            message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::CancelRequested
        })
        .unwrap();
    assert_eq!(
        cleanup.predecessor_message_id,
        Some(first_request.message_id)
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
        .prepare_join_before_network_without_route(
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
        .prepare_join_before_network_without_route(
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
async fn failed_new_request_delivery_keeps_replacement_current_for_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xfd; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let preparation = RotatingPreparation {
        calls: AtomicUsize::new(0),
    };
    let first = owner
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"first-request",
            false,
        )
        .await
        .unwrap();
    let replacement = owner
        .prepare_join_before_network_without_route(
            &preparation,
            &DeviceId::new("joiner"),
            b"second-sponsor",
            b"second-request",
            false,
        )
        .await
        .unwrap();

    let deferred = owner
        .recover_with(&DeferredAdmissionDelivery)
        .await
        .unwrap();
    assert_eq!(deferred.deliveries_attempted, 2);
    assert_eq!(deferred.deliveries_confirmed, 0);
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        replacement.attempt.attempt_id
    );
    assert_eq!(
        repository
            .load(first.attempt.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
    );
    assert!(
        !repository
            .load(replacement.attempt.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .outboxes[0]
            .superseded
    );

    let reopened = durable_admission_owner(Arc::clone(&repository));
    let recovered = reopened
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();
    assert_eq!(recovered.deliveries_attempted, 2);
    assert_eq!(recovered.deliveries_confirmed, 2);
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        replacement.attempt.attempt_id
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
    repository
        .compare_and_replace_membership_history(None, b"source-membership-history")
        .await
        .unwrap();
    let first = owner
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"first-sponsor",
            b"initial-sponsor-address",
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
    let (candidate_payload, _, candidate_event, _, _) =
        durable_candidate_verification_fixture(first.attempt.attempt_id);
    candidate.candidate_event = Some(postcard::to_stdvec(&candidate_event).unwrap());
    candidate.target_relationships = Some(candidate_payload.target_relationships);
    repository
        .compare_and_advance(first.attempt.attempt_id, 0, &candidate)
        .await
        .unwrap();

    let second = owner
        .prepare_join_before_network_without_route(
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
    let previous = repository
        .load(first.attempt.attempt_id)
        .await
        .unwrap()
        .unwrap();
    assert!(previous.space_transition.is_none());
    assert_eq!(previous.rejection_reason, None);
    assert_eq!(
        repository.load_membership_history().await.unwrap(),
        Some(b"source-membership-history".to_vec())
    );

    let delivery = RecordingAdmissionDelivery::default();
    let report = owner.recover_with(&delivery).await.unwrap();
    assert_eq!(report.deliveries_attempted, 2);
    assert_eq!(report.deliveries_confirmed, 2);
    assert_eq!(report.attempts_compacted, 1);
    assert!(delivery.routes.lock().unwrap().contains(&(
        first.attempt.attempt_id,
        Some(crate::deps::AdmissionOutboxDeliveryRouteV1::Continuation(
            vec![5]
        ),),
    )));
    assert_eq!(
        repository
            .project_current_local_join()
            .await
            .unwrap()
            .unwrap()
            .attempt_id,
        second.attempt.attempt_id
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
        .prepare_join_before_network_without_route(
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
            .prepare_join_before_network_without_route(
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
async fn explicit_join_after_prepared_rejects_every_space_transition_mode() {
    use uc_core::membership::{
        AdmissionSpaceTransitionV2, CrossSpaceTransitionPhaseV2, CrossSpaceTransitionV2,
        FreshSpaceTransitionPhaseV1, FreshSpaceTransitionV1, SameSpaceTransitionPhaseV1,
        SameSpaceTransitionV1, CROSS_SPACE_TRANSITION_FORMAT_V2, FRESH_SPACE_TRANSITION_FORMAT_V1,
        SAME_SPACE_TRANSITION_FORMAT_V1,
    };

    for mode in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let repository = durable_admission_repository(&directory, [0x80 + mode; 16]);
        let owner = durable_admission_owner(Arc::clone(&repository));
        let preparation = RotatingPreparation {
            calls: AtomicUsize::new(0),
        };
        let first = owner
            .prepare_join_before_network_without_route(
                &preparation,
                &DeviceId::new("joiner"),
                b"first-sponsor",
                b"first-request",
                false,
            )
            .await
            .unwrap();
        let attempt_id = first.attempt.attempt_id;
        let mut prepared = first.attempt.clone();
        prepared.record_version = 1;
        advance_joiner_to_candidate_or_prepared(
            &mut prepared,
            uc_core::membership::JoinerAdmissionStageV1::Prepared,
        );
        let transition = match mode {
            0 => AdmissionSpaceTransitionV2::Fresh(FreshSpaceTransitionV1 {
                transition_format_version: FRESH_SPACE_TRANSITION_FORMAT_V1,
                attempt_id,
                target_space_id: "target-space".to_owned(),
                target_generation: [1; 16],
                target_keyslot_ref: vec![2],
                target_workspace_ref: vec![3],
                phase: FreshSpaceTransitionPhaseV1::TargetStaged,
            }),
            1 => AdmissionSpaceTransitionV2::SameSpace(SameSpaceTransitionV1 {
                transition_format_version: SAME_SPACE_TRANSITION_FORMAT_V1,
                attempt_id,
                target_space_id: "target-space".to_owned(),
                source_generation: [4; 16],
                target_generation: [5; 16],
                target_keyslot_ref: vec![6],
                target_workspace_ref: vec![7],
                phase: SameSpaceTransitionPhaseV1::TargetStaged,
            }),
            _ => AdmissionSpaceTransitionV2::CrossSpace(CrossSpaceTransitionV2 {
                transition_format_version: CROSS_SPACE_TRANSITION_FORMAT_V2,
                attempt_id,
                source_space_id: "source-space".to_owned(),
                source_generation: [8; 16],
                source_backup_ref: vec![9],
                source_backup_digest: [10; 32],
                source_revision_at_backup: 1,
                target_space_id: "target-space".to_owned(),
                target_generation: [11; 16],
                target_keyslot_ref: vec![12],
                target_workspace_ref: vec![13],
                phase: CrossSpaceTransitionPhaseV2::TargetStaged,
                final_source_revision: None,
                final_manifest_digest: None,
                migrated_records: 0,
                preserved_unreadable_records: 0,
                preserve_unreadable_history: false,
            }),
        };
        prepared.space_transition = transition.encode();
        repository
            .compare_and_advance(attempt_id, 0, &prepared)
            .await
            .unwrap();
        let metadata_before = repository.profile_metadata().await.unwrap();

        assert!(matches!(
            owner
                .prepare_join_before_network_without_route(
                    &preparation,
                    &DeviceId::new("joiner"),
                    b"second-sponsor",
                    b"second-request",
                    false,
                )
                .await,
            Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded)
        ));
        assert_eq!(repository.load(attempt_id).await.unwrap(), Some(prepared));
        assert_eq!(
            repository.profile_metadata().await.unwrap(),
            metadata_before
        );
    }
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
        first_owner.prepare_join_before_network_without_route(
            &first_preparation,
            &first_device,
            b"first-sponsor",
            b"first-request",
            false,
        ),
        second_owner.prepare_join_before_network_without_route(
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
        owner.prepare_join_before_network_without_route(
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
    let started = seed_initiated_join(repository.as_ref(), attempt_id, [0x26; 16]).await;
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
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x28; 32]);
    let started = seed_initiated_join(repository.as_ref(), attempt_id, [0x29; 16]).await;
    let mut wrong = super::admission::admission_acknowledgment(&started.outboxes[0]);
    wrong.payload_digest[0] ^= 0xff;

    assert!(
        crate::space::admission::recover_pending_admissions::record_protocol_message_delivered(
            repository.as_ref(),
            attempt_id,
            &wrong,
        )
        .await
        .is_err()
    );
    assert!(!repository.load(attempt_id).await.unwrap().unwrap().outboxes[0].superseded);

    let exact = super::admission::admission_acknowledgment(&started.outboxes[0]);
    crate::space::admission::recover_pending_admissions::record_protocol_message_delivered(
        repository.as_ref(),
        attempt_id,
        &exact,
    )
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
    let owner = WorkspaceMembership::new(deps);
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
    let owner = WorkspaceMembership::new(deps);
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
    let mut state = SpaceMembershipState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(sponsor_member);
    workspace_repository.save_state(&state).await.unwrap();
    let security = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut deps = test_deps(
        Arc::new(workspace_repository),
        sponsor_device.as_str(),
        Vec::new(),
    );
    deps.membership_history_repo = Arc::clone(&admission_repository);
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
    let owner = WorkspaceMembership::new(deps);

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
    let mut state = SpaceMembershipState::fresh(SPACE.to_owned(), 1);
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
    let owner = WorkspaceMembership::new(deps);
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
    let admission_repository = durable_admission_repository(&directory, [0x71; 16]);
    deps.membership_history_repo = Arc::clone(&admission_repository);
    deps.admission_attempts = admission_repository;
    deps.legacy_migration_recovery = recovery.clone();
    let owner = WorkspaceMembership::new(deps);
    let recovery_use_case = crate::space::admission::RecoverPendingAdmissionsUseCase::new(
        crate::space::admission::SpaceAdmission::new(owner),
    );

    recovery_use_case.execute().await.unwrap();

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
    let joiner_transaction = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = AdmissionAttemptId::from_bytes([0x74; 32]);
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let initiated = seed_initiated_join(joiner_repository.as_ref(), attempt_id, [0x75; 16]).await;
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
    crate::space::admission::joiner::record_invitation_consume_result(
        sponsor_repository.as_ref(),
        attempt_id,
        crate::space::admission::joiner::InvitationConsumeResultV1::Consumed,
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
        .load_membership_history()
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
    first_deps.membership_history_repo = Arc::clone(&racing_repository);
    first_deps.admission_attempts = racing_repository;
    first_deps.activate_sponsor_admission_security = first_activation.clone();
    first_deps.member_repo = member_repo.clone();
    first_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential.clone(),
    });
    let first_owner = WorkspaceMembership::new(first_deps);

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
    resumed_deps.membership_history_repo = Arc::clone(&sponsor_repository);
    resumed_deps.admission_attempts = Arc::clone(&sponsor_repository);
    resumed_deps.activate_sponsor_admission_security = resumed_activation.clone();
    resumed_deps.member_repo = member_repo.clone();
    resumed_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    let resumed_owner = WorkspaceMembership::new(resumed_deps);
    let recovery_use_case = crate::space::admission::RecoverPendingAdmissionsUseCase::new(
        crate::space::admission::SpaceAdmission::new(resumed_owner),
    );

    assert_eq!(recovery_use_case.execute().await.unwrap(), 1);
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
            .load_membership_history()
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

    recovery_use_case.execute().await.unwrap();
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
        .submit_legacy_removal_for_test(&DeviceId::new("device-b"))
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
    let mut state = SpaceMembershipState::fresh(SPACE.to_owned(), 1);
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
    let mut state = SpaceMembershipState::fresh(SPACE.to_owned(), 1);
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
