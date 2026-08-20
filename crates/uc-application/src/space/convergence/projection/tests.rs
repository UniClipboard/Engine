use super::super::tests::*;

#[tokio::test]
async fn upgrade_required_peer_remains_blocked_after_owner_restart() {
    let a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let first = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        vec![(DeviceId::new("device-a"), a)],
    ));
    let peer = DeviceId::new("joiner");
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: peer.clone(),
                relationship: uc_core::membership::MembershipHistoryRelationship::UpgradeRequired,
            },
            2,
        )
        .unwrap();
    repository.save_state(&state).await.unwrap();
    assert!(first.locally_removed(&peer).await);

    let restarted = WorkspaceConvergence::new(test_deps(
        Arc::new(repository),
        "device-a",
        vec![(DeviceId::new("device-a"), a)],
    ));

    assert!(restarted.locally_removed(&peer).await);
    assert_eq!(
        restarted
            .query()
            .await
            .unwrap()
            .upgrade_required_peer_device_ids,
        vec![peer]
    );
}

#[tokio::test]
async fn profile_device_trust_is_explicitly_unavailable_while_active_space_is_locked() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x6d; 16]);
    let mut deps = test_deps(Arc::new(LockedWorkspaceRepository), "device-a", Vec::new());
    deps.admission_attempts = Arc::clone(&admission_repository)
        as Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>;
    let active = WorkspaceConvergence::new(deps);
    let profile = super::ProfileWorkspaceConvergence::new(
        admission_repository,
        DeviceId::new("device-a"),
        Arc::new(UnusedClock),
    );
    profile.attach_active(Some(active)).await;

    let snapshot = profile.query_device_trust().await.unwrap();

    assert_eq!(snapshot.local_device_id, DeviceId::new("device-a"));
    assert_eq!(
        snapshot.local_membership,
        super::DeviceMembership::Unavailable
    );
    assert!(snapshot.devices.is_empty());
    assert_eq!(
        snapshot.blocked_reason,
        Some(super::ActionUnavailableReason::EngineUnavailable)
    );
}

#[tokio::test]
async fn pending_inbound_projection_shows_only_the_active_lineage_non_terminal_candidate() {
    use uc_core::membership::AdmissionRejectionReasonV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x74; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x75; 16]);
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x76; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x77; 16],
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
            [0x78; 32],
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

    assert_eq!(
        sponsor.pending_inbound_member("space-b").await.unwrap(),
        None
    );
    let projected = sponsor
        .pending_inbound_member("space-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected.device_id, DeviceId::new("joiner"));
    assert_eq!(projected.display_name, "joiner");

    sponsor
        .sponsor_reject_before_commit(
            attempt_id,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();
    assert_eq!(
        sponsor.pending_inbound_member("space-a").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn device_trust_query_reports_a_consistent_compatible_peer_as_usable() {
    use crate::space::convergence::SyncRelationship;

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
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Consistent,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.sync_relationship, SyncRelationship::Usable);
}

#[tokio::test]
async fn device_trust_query_keeps_reachability_independent_from_a_usable_relationship() {
    use crate::space::convergence::{GroupRelationship, SyncRelationship};

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    harness
        .presence
        .states
        .lock()
        .unwrap()
        .insert(DeviceId::new("device-a"), ReachabilityState::Offline);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Consistent,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.reachability, ReachabilityState::Offline);
    assert_eq!(peer.group_relationship, GroupRelationship::Consistent);
    assert_eq!(peer.sync_relationship, SyncRelationship::Usable);
    assert!(snapshot.current_change.is_none());
}

#[tokio::test]
async fn device_trust_query_reports_invalid_peer_facts_as_unverifiable_and_paused() {
    use crate::space::convergence::{ActionUnavailableReason, GroupRelationship, SyncRelationship};

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
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Invalid,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.group_relationship, GroupRelationship::Unverifiable);
    assert_eq!(peer.sync_relationship, SyncRelationship::PausedUnverifiable);
    assert_eq!(
        peer.blocked_reason,
        Some(ActionUnavailableReason::DeviceFactsUnverifiable)
    );
}

#[tokio::test]
async fn device_trust_query_fails_closed_when_the_workspace_facts_are_unverifiable() {
    use crate::space::convergence::{ActionUnavailableReason, GroupRelationship, SyncRelationship};

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
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Consistent,
    );
    state.failure_category = Some(uc_core::membership::WorkspaceFailureCategory::DigestConflict);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.group_relationship, GroupRelationship::Unverifiable);
    assert_eq!(peer.sync_relationship, SyncRelationship::PausedUnverifiable);
    assert_eq!(
        snapshot.blocked_reason,
        Some(ActionUnavailableReason::DeviceFactsUnverifiable)
    );
    assert!(snapshot.allowed_actions.is_empty());
    assert!(snapshot.current_change.is_none());
}

#[tokio::test]
async fn content_gate_blocks_only_pending_or_diverged_history_peers() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let pending = DeviceId::new("device-pending");
    let unaffected = DeviceId::new("device-unaffected");
    let pending_instance = instance(0x0c);
    let unaffected_instance = instance(0x0d);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let pending_addition = membership_event(
        Some(genesis.event_id()),
        1,
        a,
        pending_instance,
        pending.as_str(),
        2,
    );
    let unaffected_addition = membership_event(
        Some(pending_addition.event_id()),
        2,
        a,
        unaffected_instance,
        unaffected.as_str(),
        3,
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    for event in [genesis, pending_addition, unaffected_addition] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: pending,
                relationship:
                    uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
            },
            2,
        )
        .unwrap();
    harness.repository.save_state(&state).await.unwrap();

    assert!(harness.owner.locally_removed(&pending).await);
    assert!(!harness.owner.locally_removed(&unaffected).await);
}

#[tokio::test]
async fn current_peer_scope_fails_closed_when_v2_history_is_corrupt() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x78; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x79; 32]);
    let mut attempt = uc_core::membership::AdmissionAttemptV1::new_joiner(
        attempt_id,
        [0x7a; 16],
        uc_core::membership::JoinerAdmissionStageV1::Initiated,
    );
    attempt.join_id = None;
    attempt.local_join_ordinal = None;
    attempt.role_state = uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(
        uc_core::membership::SponsorAdmissionStateV1 {
            stage: uc_core::membership::SponsorAdmissionStageV1::Accepted,
        },
    );
    attempt.invitation_claim = Some(b"scope-invitation".to_vec());
    admission_repository
        .create(&attempt, None, Some(b"corrupt-history"))
        .await
        .unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Corrupt)
    );
}

#[tokio::test]
async fn pending_cross_space_join_keeps_the_source_space_scope() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x7b; 16]);
    seed_v2_scope_history_for_lineage(
        Arc::clone(&admission_repository),
        "target-space",
        true,
        true,
        Some(SPACE),
    )
    .await;
    let own = instance(0x0a);
    let peer = instance(0x0b);
    let genesis = membership_event(None, 0, own, own, "joiner", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, own, peer, "source-peer", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), own);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own);
    state.membership_reconciliation = Some(history);
    let repository = MemoryWorkspaceRepository::default();
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("source-peer")]);
}

#[tokio::test]
async fn unrelated_pending_join_does_not_hide_a_v2_lineage_mismatch() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x7c; 16]);
    seed_v2_scope_history_for_lineage(
        Arc::clone(&admission_repository),
        "unrelated-space",
        true,
        true,
        None,
    )
    .await;
    let own = instance(0x0a);
    let genesis = membership_event(None, 0, own, own, "joiner", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), own);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own);
    state.membership_reconciliation = Some(history);
    let repository = MemoryWorkspaceRepository::default();
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Corrupt)
    );
}

#[tokio::test]
async fn rejected_cross_space_join_restores_the_source_space_scope() {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionRejectionReasonV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, JoinerAdmissionStateV1,
    };

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x80; 16]);
    seed_v2_scope_history_for_lineage(
        Arc::clone(&admission_repository),
        "target-space",
        true,
        true,
        Some(SPACE),
    )
    .await;
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x75; 32]);
    let mut rejected = admission_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap();
    let expected_version = rejected.record_version;
    rejected.record_version += 1;
    rejected.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
        stage: JoinerAdmissionStageV1::Rejected,
    });
    rejected.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
    rejected.rejection_reason = Some(AdmissionRejectionReasonV1::RemovedBeforeActivation);
    rejected.identity_binding = Some(b"joiner-identity".to_vec());
    rejected.space_transition = None;
    rejected.target_access_state = None;
    rejected.staged_security_state = None;
    admission_repository
        .compare_and_advance(attempt_id, expected_version, &rejected)
        .await
        .unwrap();
    admission_repository
        .compact_terminal(attempt_id, rejected.record_version)
        .await
        .unwrap();

    let own = instance(0x0a);
    let peer = instance(0x0b);
    let genesis = membership_event(None, 0, own, own, "joiner", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, own, peer, "source-peer", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), own);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own);
    state.membership_reconciliation = Some(history);
    let repository = MemoryWorkspaceRepository::default();
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("source-peer")]);
}

#[tokio::test]
async fn device_trust_query_returns_complete_pending_change_and_per_device_relationships() {
    use crate::space::convergence::{
        DeviceCompatibility, DeviceMembership, GroupRelationship, SyncRelationship,
    };

    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    harness.presence.states.lock().unwrap().extend([
        (DeviceId::new("device-a"), ReachabilityState::Online),
        (DeviceId::new("device-b"), ReachabilityState::Offline),
    ]);
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
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, b_addition, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let change = snapshot.current_change.expect("one current change");
    assert_eq!(change.change_id, removal.event_id());
    assert_eq!(change.proposed_by_device_id, DeviceId::new("device-a"));
    assert_eq!(change.target_device_ids, vec![DeviceId::new("device-b")]);
    assert!(!change.includes_local_device);
    assert!(change
        .apply_impact
        .requires_rejoin_device_ids
        .contains(&DeviceId::new("device-b")));
    assert!(change
        .keep_current_impact
        .paused_device_ids
        .contains(&DeviceId::new("device-a")));

    let a_view = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(a_view.reachability, ReachabilityState::Online);
    assert_eq!(a_view.membership, DeviceMembership::Active);
    assert_eq!(
        a_view.group_relationship,
        GroupRelationship::PendingLocalDecision
    );
    assert_eq!(a_view.compatibility, DeviceCompatibility::Compatible);
    assert_eq!(
        a_view.sync_relationship,
        SyncRelationship::WaitingForLocalDecision
    );

    let b_view = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-b"))
        .unwrap();
    assert_eq!(b_view.reachability, ReachabilityState::Offline);
    assert_eq!(b_view.membership, DeviceMembership::Active);
    assert_eq!(b_view.group_relationship, GroupRelationship::Unknown);
}

#[tokio::test]
async fn v2_current_peer_scope_opens_for_an_observer_after_activation_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x73; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, false).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("joiner")]);
    assert!(!owner.locally_removed(&DeviceId::new("joiner")).await);
}

#[tokio::test]
async fn v2_joiner_scope_stays_closed_until_the_local_join_is_active() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x74; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, true).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Removed
    );
    assert!(snapshot.peer_device_ids.is_empty());
}

#[tokio::test]
async fn v2_joiner_scope_opens_after_the_local_join_is_active() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x77; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, false).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("sponsor")]);
}

#[tokio::test]
async fn rejected_local_join_does_not_remove_an_existing_current_member() {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionRejectionReasonV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, JoinerAdmissionStateV1,
    };

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x81; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, true).await;
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x75; 32]);
    let mut rejected = admission_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap();
    let expected_version = rejected.record_version;
    rejected.record_version += 1;
    rejected.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
        stage: JoinerAdmissionStageV1::Rejected,
    });
    rejected.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
    rejected.rejection_reason = Some(AdmissionRejectionReasonV1::HistoryConflict);
    admission_repository
        .compare_and_advance(attempt_id, expected_version, &rejected)
        .await
        .unwrap();
    admission_repository
        .compact_terminal(attempt_id, rejected.record_version)
        .await
        .unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("sponsor")]);
}

#[tokio::test]
async fn current_peer_scope_fails_closed_when_v2_history_is_locked() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = Arc::new(LockedAdmissionRepository {
        allow_empty_history_reads: false,
    });
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Locked)
    );
}

#[tokio::test]
async fn device_trust_query_returns_a_migrated_workspace_as_upgrade_required() {
    use crate::space::convergence::{DeviceCompatibility, SyncRelationship};

    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(instance(0x0a));
    state.migrated_from_pre_adr_020 = true;
    state.peer_history_relationships.insert(
        DeviceId::new("device-b"),
        uc_core::membership::MembershipHistoryRelationship::UpgradeRequired,
    );
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-b"))
        .unwrap();

    assert_eq!(snapshot.local_device_id, DeviceId::new("device-a"));
    assert_eq!(snapshot.devices.len(), 2);
    assert_eq!(peer.compatibility, DeviceCompatibility::UpgradeRequired);
    assert_eq!(
        peer.sync_relationship,
        SyncRelationship::PausedUpgradeRequired
    );
}

#[tokio::test]
async fn current_peer_scope_does_not_infer_legacy_mode_from_missing_history() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Ready));
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Unavailable)
    );
}

#[tokio::test]
async fn current_peer_scope_hides_addition_until_pending_effects_finish() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness("device-a", Vec::new());
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
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert!(snapshot.peer_device_ids.is_empty());
}

#[tokio::test]
async fn v2_current_peer_scope_requires_a_permanent_activation_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x72; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), false, false).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert!(snapshot.peer_device_ids.is_empty());
}

#[tokio::test]
async fn current_peer_scope_accepts_a_legacy_roster_that_only_stores_remote_members() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Legacy));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}

#[tokio::test]
async fn device_trust_uses_the_legacy_scope_for_a_fresh_workspace() {
    use crate::space::convergence::DeviceMembership;

    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-a")]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Legacy));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.query_device_trust().await.unwrap();

    assert_eq!(snapshot.local_membership, DeviceMembership::Active);
    assert_eq!(snapshot.devices.len(), 1);
    assert_eq!(snapshot.devices[0].membership, DeviceMembership::Active);
}

#[tokio::test]
async fn device_trust_does_not_infer_membership_without_legacy_or_current_history() {
    use crate::space::convergence::DeviceMembership;

    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-a")]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Ready));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.query_device_trust().await.unwrap();

    assert_eq!(snapshot.local_membership, DeviceMembership::Unavailable);
    assert_eq!(
        snapshot.devices[0].membership,
        DeviceMembership::Unavailable
    );
}

#[tokio::test]
async fn current_peer_scope_keeps_a_migrated_pre_adr_020_workspace_in_legacy_upgrade() {
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(instance(0x0a));
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::default());
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::Legacy
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}

#[tokio::test]
async fn migrated_remote_only_roster_checks_local_protection_before_membership() {
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let protection = Arc::new(ProtectsQueriedMembers::default());
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    deps.space_protection = protection.clone();
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
    assert_eq!(
        protection.queries.lock().unwrap().as_slice(),
        &[vec![DeviceId::new("device-a"), DeviceId::new("device-b")]]
    );
}

#[tokio::test]
async fn workspace_query_uses_the_persisted_v2_history_as_its_current_truth() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x72; 16]);
    let (history, _, _) = admission_verification_fixture_for_lineage([0x73; 32], SPACE);
    let encoded_history = history.encode_persisted_v2().unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &encoded_history)
        .await
        .unwrap();
    let repository = MemoryWorkspaceRepository::default();
    repository
        .save_state(&WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1))
        .await
        .unwrap();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);
    let expected_position = history.current_position().unwrap();

    let snapshot = owner.query().await.unwrap();

    assert_eq!(
        snapshot.history_event_count,
        usize::try_from(expected_position.depth.saturating_add(1)).unwrap()
    );
    assert_eq!(snapshot.effective_member_count, 1);
    assert_eq!(
        snapshot.convergence_digest,
        Some(uc_core::membership::WorkspaceDigest::from_bytes(
            expected_position.history_digest
        ))
    );
}

#[tokio::test]
async fn current_peer_scope_excludes_an_accepted_removal() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness("device-a", Vec::new());
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
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-c")]);
}

#[tokio::test]
async fn current_peer_scope_keeps_a_removal_pending_local_decision() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness("device-b", Vec::new());
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
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
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    for event in [genesis, addition, removal] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-a")]);
}

#[tokio::test]
async fn current_peer_scope_is_empty_after_local_removal() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness("device-b", Vec::new());
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.removed = true;
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert!(snapshot.peer_device_ids.is_empty());
    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Removed
    );
}

#[tokio::test]
async fn current_peer_scope_uses_legacy_members_only_in_explicit_legacy_mode() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Legacy));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::Legacy
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}
