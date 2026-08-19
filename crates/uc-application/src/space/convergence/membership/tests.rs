use super::super::tests::*;

#[tokio::test]
async fn unknown_v2_member_may_introduce_a_complete_activated_extension() {
    use uc_core::membership::{MembershipHistoryMessage, MembershipHistoryV2Ack};

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0xa3; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xa4; 32]);
    let (_, base_history, candidate, _, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let mut incoming = base_history.clone();
    incoming
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    incoming
        .verify_and_record_activation_receipt(activation_receipt, &DeterministicHistoricalVerifier)
        .unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(
            None,
            &base_history.encode_persisted_v2().unwrap(),
        )
        .await
        .unwrap();

    let sponsor_credential = base_history
        .credential_for(candidate.author_member_instance_id)
        .unwrap()
        .clone();
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = &candidate.operation
    else {
        unreachable!("fixture always creates AddDevice")
    };
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(candidate.lineage_id.clone(), 1);
    state.own_instance = Some(candidate.author_member_instance_id);
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    let owner = WorkspaceConvergence::new(deps);

    assert!(incoming.is_complete_extension_of(&base_history));
    assert!(incoming
        .active_members()
        .contains(&admission.facts.member_instance));
    let mut sender_facts = admission.facts.clone();
    sender_facts.identity_signature = DeterministicHistoricalVerifier.sign(
        &admission.membership_credential,
        &sender_facts.signing_payload(),
    );
    let pages = incoming
        .export_reconciliation_pages_v2(sender_facts)
        .unwrap();
    let imported = uc_core::membership::VersionedMembershipHistory::import_exchange_pages_v2(
        &pages,
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(imported, incoming);

    let response = owner
        .handle_membership_history(
            &admission.facts.device_id,
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::UpdatesApplied)
    );
    assert_eq!(
        owner.query().await.unwrap().history_event_count,
        usize::try_from(
            base_history
                .current_position()
                .unwrap()
                .depth
                .saturating_add(2)
        )
        .unwrap()
    );
}

#[tokio::test]
async fn paged_history_resumes_after_restart_and_applies_only_when_complete() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0xb1; 16]);
    let (base, incoming, pages, _sponsor_credential, joiner_credential) =
        paged_runtime_history_fixture(0x3c1);
    let base_bytes = base.encode_persisted_v2().unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &base_bytes)
        .await
        .unwrap();
    let workspace_repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(joiner_credential.member_instance_id(&DeviceId::new("joiner")));
    workspace_repository.save_state(&state).await.unwrap();
    let receiver = paged_receiver(
        workspace_repository.clone(),
        admission_repository.clone(),
        joiner_credential.clone(),
    );

    let early = receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[1].clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        early,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue {
            transfer_id: pages[0].transfer_id(),
            next_page_index: 0,
        })
    );
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(base_bytes.clone())
    );

    let first = receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        first,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue {
            transfer_id: pages[0].transfer_id(),
            next_page_index: 1,
        })
    );
    drop(receiver);

    let restarted = paged_receiver(
        workspace_repository.clone(),
        admission_repository.clone(),
        joiner_credential,
    );
    let duplicate = restarted
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();
    assert_eq!(duplicate, first);
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(base_bytes)
    );

    let completed = restarted
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[1].clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        completed,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::UpdatesApplied)
    );
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(incoming.encode_persisted_v2().unwrap())
    );
    assert!(workspace_repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_membership_history_transfers
        .is_empty());
}

#[tokio::test]
async fn paged_history_rejects_a_conflicting_transfer_and_clears_progress() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0xb2; 16]);
    let (base, _incoming, pages, _sponsor_credential, joiner_credential) =
        paged_runtime_history_fixture(0x3c2);
    let (_, _, conflicting_pages, _, _) = paged_runtime_history_fixture(0x3c3);
    let base_bytes = base.encode_persisted_v2().unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &base_bytes)
        .await
        .unwrap();
    let workspace_repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(joiner_credential.member_instance_id(&DeviceId::new("joiner")));
    workspace_repository.save_state(&state).await.unwrap();
    let receiver = paged_receiver(
        workspace_repository.clone(),
        admission_repository.clone(),
        joiner_credential,
    );
    receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();

    let response = receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(conflicting_pages[0].clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Invalid)
    );
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(base_bytes)
    );
    assert!(workspace_repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_membership_history_transfers
        .is_empty());
}

#[tokio::test]
async fn paged_history_transfers_257_events_end_to_end() {
    let receiver_directory = tempfile::tempdir().unwrap();
    let receiver_admission = durable_admission_repository(&receiver_directory, [0xb3; 16]);
    let (base, incoming, _pages, sponsor_credential, joiner_credential) =
        paged_runtime_history_fixture(0x3c4);
    receiver_admission
        .compare_and_replace_membership_history_v2(None, &base.encode_persisted_v2().unwrap())
        .await
        .unwrap();
    let receiver_workspace = MemoryWorkspaceRepository::default();
    let mut receiver_state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    receiver_state.own_instance =
        Some(joiner_credential.member_instance_id(&DeviceId::new("joiner")));
    receiver_workspace
        .save_state(&receiver_state)
        .await
        .unwrap();
    let receiver = paged_receiver(
        receiver_workspace,
        receiver_admission.clone(),
        joiner_credential,
    );
    let loopback = Arc::new(LoopbackHistoryExchange {
        receiver,
        source_device_id: DeviceId::new("sponsor"),
        sent_pages: AtomicUsize::new(0),
    });

    let sender_directory = tempfile::tempdir().unwrap();
    let sender_admission = durable_admission_repository(&sender_directory, [0xb4; 16]);
    sender_admission
        .compare_and_replace_membership_history_v2(None, &incoming.encode_persisted_v2().unwrap())
        .await
        .unwrap();
    let sender_workspace = MemoryWorkspaceRepository::default();
    let mut sender_state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    sender_state.own_instance =
        Some(sponsor_credential.member_instance_id(&DeviceId::new("sponsor")));
    sender_workspace.save_state(&sender_state).await.unwrap();
    let mut sender_deps = test_deps(Arc::new(sender_workspace), "sponsor", Vec::new());
    sender_deps.admission_attempts = sender_admission.clone();
    sender_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    sender_deps.membership_identity = Arc::new(FixedMembershipIdentity {
        space: SpaceId::from_str(SPACE),
        device_id: DeviceId::new("sponsor"),
    });
    sender_deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: DeviceId::new("sponsor"),
    });
    sender_deps.membership_history_exchange = loopback.clone();
    sender_deps.own_device = DeviceId::new("sponsor");
    let sender = WorkspaceConvergence::new(sender_deps);

    sender
        .reconcile_membership_history_with_peer(&DeviceId::new("joiner"))
        .await
        .unwrap();

    assert_eq!(loopback.sent_pages.load(Ordering::SeqCst), 2);
    let sender_history = sender_admission.load_membership_history_v2().await.unwrap();
    let receiver_history = receiver_admission
        .load_membership_history_v2()
        .await
        .unwrap();
    assert_eq!(receiver_history, sender_history);
}

#[tokio::test]
async fn concurrent_online_events_run_one_reconciliation_per_peer() {
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(BlockingTrackingExchange::new());
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    let own_instance = install_current_history(&mut deps, &directory, 0xb5).await;
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
    repository.save_state(&state).await.unwrap();

    let first_owner = Arc::clone(&owner);
    let first_peer = peer.clone();
    let first = tokio::spawn(async move {
        first_owner
            .reconcile_membership_history_with_peer(&first_peer)
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        exchange.started.notified(),
    )
    .await
    .expect("first reconciliation starts");

    let second_owner = Arc::clone(&owner);
    let second_peer = peer.clone();
    let second = tokio::spawn(async move {
        second_owner
            .reconcile_membership_history_with_peer(&second_peer)
            .await
    });
    tokio::task::yield_now().await;
    assert_eq!(exchange.calls.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.maximum_active.load(Ordering::SeqCst), 1);

    exchange.releases.add_permits(2);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(exchange.calls.load(Ordering::SeqCst), 2);
    assert_eq!(exchange.maximum_active.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn pre_admission_history_synchronization_uses_one_ten_second_total_budget() {
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(BlockingTrackingExchange::new());
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    deps.peer_addr_repo = Arc::new(FixedPeerAddrRepo {
        records: vec![
            uc_core::ports::PeerAddressRecord {
                device_id: DeviceId::new("offline-a"),
                addr_blob: vec![1],
                observed_at: chrono::Utc::now(),
            },
            uc_core::ports::PeerAddressRecord {
                device_id: DeviceId::new("offline-b"),
                addr_blob: vec![2],
                observed_at: chrono::Utc::now(),
            },
        ],
    });
    let own_instance = install_current_history(&mut deps, &directory, 0xc6).await;
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
    repository.save_state(&state).await.unwrap();

    let first_request_started = exchange.started.notified();
    let synchronization = tokio::spawn({
        let owner = Arc::clone(&owner);
        async move { owner.synchronize_chain().await }
    });
    first_request_started.await;

    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    tokio::task::yield_now().await;

    assert!(
        synchronization.is_finished(),
        "all pre-admission history checks must share a ten-second budget"
    );
    assert!(synchronization.await.unwrap().is_ok());
    assert_eq!(exchange.calls.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.maximum_active.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn persisted_v2_removal_decision_is_retried_after_restart_for_a_diverged_author() {
    use uc_core::membership::{
        MembershipDecisionV2, MembershipOperationV2, MEMBERSHIP_DECISION_FORMAT_V2,
    };

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x93; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x94; 32]);
    let (_, mut local_history, candidate, _, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let MembershipOperationV2::AddDevice { admission } = &candidate.operation else {
        unreachable!("fixture always creates AddDevice")
    };
    let local_credential = admission.membership_credential.clone();
    let local_member = admission.facts.member_instance;
    local_history
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    local_history
        .verify_and_record_activation_receipt(activation_receipt, &DeterministicHistoricalVerifier)
        .unwrap();
    let removal = durable_candidate_removal_fixture(attempt_id);
    let mut author_history = local_history.clone();
    author_history
        .verify_and_receive_event(removal.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    local_history
        .merge_remote_history(
            &author_history,
            local_member,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    let mut rejection = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        removal.lineage_id.clone(),
        removal.event_id(),
        local_member,
        local_credential.credential_id,
        local_credential.signature_algorithm_version,
        RemovalDecision::Reject,
        removal.parent_event_id,
        candidate.resulting_members_digest,
        [0x95; 16],
        Vec::new(),
    );
    rejection.signature =
        DeterministicHistoricalVerifier.sign(&local_credential, &rejection.signing_payload());
    local_history
        .verify_and_record_local_decision(rejection, local_member, &DeterministicHistoricalVerifier)
        .unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(
            None,
            &local_history.encode_persisted_v2().unwrap(),
        )
        .await
        .unwrap();

    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(removal.lineage_id.clone(), 1);
    state.own_instance = Some(local_member);
    state.peer_history_relationships.insert(
        DeviceId::new("sponsor"),
        uc_core::membership::MembershipHistoryRelationship::Diverged,
    );
    repository.save_state(&state).await.unwrap();
    let exchange = Arc::new(ScriptedExchange::new(vec![
        MembershipHistoryMessage::AckV2(
            uc_core::membership::MembershipHistoryV2Ack::UpdatesApplied,
        ),
    ]));
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("joiner"),
        credential: local_credential,
    });
    deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: DeviceId::new("joiner"),
    });
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("sponsor")]));
    deps.membership_history_exchange = exchange.clone();
    let restarted = WorkspaceConvergence::new(deps);

    assert!(
        restarted.locally_removed(&DeviceId::new("sponsor")).await,
        "a diverged V2 peer must remain blocked for normal content after restart"
    );

    restarted
        .deliver_pending_membership_decisions()
        .await
        .unwrap();

    let sent = exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, DeviceId::new("sponsor"));
    assert!(matches!(
        sent[0].1,
        MembershipHistoryMessage::HistoryPageV2(_)
    ));
}

#[tokio::test]
async fn concurrent_matching_device_trust_decisions_save_only_one_completion() {
    use crate::space::convergence::DeviceTrustDecisionResult;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let first_owner = Arc::clone(&harness.owner);
    let second_owner = Arc::clone(&harness.owner);
    let (first, second) = tokio::join!(
        first_owner.decide_device_trust_change(
            removal.event_id(),
            crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
            false,
        ),
        second_owner.decide_device_trust_change(
            removal.event_id(),
            crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
            false,
        ),
    );
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                DeviceTrustDecisionResult::KeptCurrentDeviceGroup { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, DeviceTrustDecisionResult::AlreadyCompleted { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn legacy_and_device_trust_decisions_share_idempotent_completion() {
    use crate::space::convergence::{DeviceTrustChoice, DeviceTrustDecisionResult};

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::KeptCurrentDeviceGroup { .. }
    ));
    assert!(harness
        .owner
        .decide_membership_removal(removal.event_id(), RemovalDecision::Reject)
        .await
        .is_ok());
}

#[tokio::test]
async fn applying_a_change_that_removes_the_local_device_requires_explicit_confirmation() {
    use crate::space::convergence::DeviceTrustDecisionResult;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: c },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::ApplyChange,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::LocalDeviceConfirmationRequired { .. }
    ));
    assert_eq!(
        harness
            .repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .membership_reconciliation
            .unwrap()
            .pending_removal_decision(),
        Some(removal.event_id())
    );
    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::ApplyChange,
                true,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::Applied { .. }
    ));
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
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis.clone()).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    let before = state.clone();
    harness.repository.save_state(&state).await.unwrap();

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
        Some(before),
        "failed removal must not change the saved state"
    );
}

#[tokio::test]
async fn membership_history_advancement_invalidates_an_older_invitation() {
    let a = instance(0x0a);
    let harness = harness("device-a", vec![(DeviceId::new("device-a"), a)]);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness.owner.admission_decision(0).await,
        MembershipAdmissionDecision::SupersededInvitation
    );
}

#[tokio::test]
async fn migrated_workspace_with_applied_history_does_not_restore_a_removed_peer() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_addition = membership_event(Some(b_addition.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        vec![4],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    for event in [genesis, b_addition, c_addition, removal] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
        legacy_member("device-c"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::default());
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-c")]);
}

#[tokio::test]
async fn runtime_clears_a_stale_legacy_marker_after_current_history_exists() {
    let a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history
        .receive_verified(membership_event(None, 0, a, a, "device-a", 1))
        .unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let owner = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        Vec::new(),
    ));
    let (_presence_tx, presence_events) = tokio::sync::broadcast::channel(1);

    let runtime = Arc::clone(&owner).start(presence_events);
    for _ in 0..100 {
        if !repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    runtime.shutdown().await;

    let repaired = repository.load_state().await.unwrap().unwrap();
    assert!(!repaired.migrated_from_pre_adr_020);

    owner.recover_legacy_migration_marker().await.unwrap();
    assert_eq!(repository.load_state().await.unwrap().unwrap(), repaired);
}

#[tokio::test]
async fn session_resume_retries_a_legacy_marker_repair_deferred_while_locked() {
    let a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history
        .receive_verified(membership_event(None, 0, a, a, "device-a", 1))
        .unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    *repository.failure.lock().unwrap() = Some(WorkspaceConvergenceRepositoryError::Locked);
    let owner = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        Vec::new(),
    ));
    let (_presence_tx, presence_events) = tokio::sync::broadcast::channel(1);
    let runtime = Arc::clone(&owner).start(presence_events);
    tokio::task::yield_now().await;

    *repository.failure.lock().unwrap() = None;
    runtime.activity().resume().await.unwrap();
    for _ in 0..100 {
        if !repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    runtime.shutdown().await;

    assert!(
        !repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
    );
}

#[tokio::test]
async fn restart_recovery_completes_and_clears_pending_membership_effects() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.pending_applied_membership_effects.push(
        uc_core::membership::PendingAppliedMembershipEffect {
            event_id: addition.event_id(),
            member_facts_completed: false,
            security_update_completed: true,
        },
    );
    repository.save_state(&state).await.unwrap();
    let owner = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        Vec::new(),
    ));

    owner.recover_pending_membership_effects().await.unwrap();

    let saved = repository.load_state().await.unwrap().unwrap();
    assert!(saved.pending_applied_membership_effects.is_empty());
    assert_eq!(
        owner.snapshot().await.unwrap().peer_device_ids,
        vec![DeviceId::new("device-b")]
    );
}

#[tokio::test]
async fn device_trust_decision_distinguishes_first_duplicate_and_conflicting_submissions() {
    use crate::space::convergence::DeviceTrustDecisionResult;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::KeptCurrentDeviceGroup { .. }
    ));
    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::AlreadyCompleted { .. }
    ));
    let restarted = WorkspaceConvergence::new(test_deps(
        Arc::new(harness.repository.clone()),
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    ));
    assert!(matches!(
        restarted
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::AlreadyCompleted { .. }
    ));
    assert!(matches!(
        restarted
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::ApplyChange,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::StateChanged { .. }
    ));
}
