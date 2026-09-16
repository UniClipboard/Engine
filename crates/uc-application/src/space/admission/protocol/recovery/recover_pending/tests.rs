use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryDisposition, AdmissionRecoveryReport,
    AdmissionRecoveryTrigger, LoadedPendingAdmission,
};
use crate::space::admission::protocol::test_support::{
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::JoinSpaceInput;
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};
use uc_core::membership::AdmissionRecordPersistence;

#[tokio::test]
async fn loaded_pending_admission_keeps_the_aggregate_and_commit_token_together() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    pair.joiner()
        .start_join_at(join_input("loaded-recovery"), 1_000)
        .await
        .expect("the join request should be saved before it can be recovered");
    let aggregate = pair.take_created_join();
    let admission_id = aggregate.admission_id();
    let token = AdmissionRecoveryCommitToken::from_bytes([0x31; 32])
        .expect("a non-zero recovery commit token is valid");

    let loaded = LoadedPendingAdmission::new(aggregate, token);
    let (aggregate, token) = loaded.into_parts();

    assert_eq!(aggregate.admission_id(), admission_id);
    assert_eq!(token.as_bytes(), &[0x31; 32]);
}

#[tokio::test]
async fn pending_join_recovery_requests_an_initial_channel_after_the_join_was_saved() {
    let output_file = tempfile::NamedTempFile::new().expect("diagnostic output");
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(output_file.reopen().expect("writer")))
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    pair.joiner()
        .start_join_at(join_input("recoverable-join"), 1_000)
        .await
        .expect("the join request should be saved before recovery");

    let report: AdmissionRecoveryReport = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.deferred_count, 1);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedJoinRequest,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInitialChannelRequested,
        ]
    );
    let diagnostics = std::fs::read_to_string(output_file.path()).expect("diagnostics");
    assert!(
        diagnostics.contains("deferred") && diagnostics.contains("state_changed"),
        "deferred recovery must explain why it remains pending"
    );
}

#[tokio::test]
async fn recovery_persists_expiry_before_attempting_more_network_work() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    pair.joiner()
        .start_join_at(join_input("expires-without-network"), 1_000)
        .await
        .expect("join is saved");

    pair.set_now_ms(300_999);
    let before = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::Periodic)
        .await;
    assert_eq!(before.terminated_count, 0);
    assert_eq!(before.deferred_count, 1);

    let network_attempts_before = pair
        .events()
        .iter()
        .filter(|event| matches!(event, ProtocolEvent::JoinerInitialChannelRequested))
        .count();
    pair.set_now_ms(301_000);
    assert_eq!(pair.saved_join().is_expired_at(301_000), Some(true));
    assert!(pair.saved_join().can_terminate_locally());
    let at_deadline = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(at_deadline.terminated_count, 1, "report: {at_deadline:?}");
    assert_eq!(pair.admission_status_invalidation_count(), 1);
    assert_eq!(
        pair.take_created_join().termination_reason(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Expired)
    );
    let network_attempts_after = pair
        .events()
        .iter()
        .filter(|event| matches!(event, ProtocolEvent::JoinerInitialChannelRequested))
        .count();
    assert_eq!(network_attempts_after, network_attempts_before);
}

#[tokio::test]
async fn authentication_rejection_diagnostic_explains_trigger_reason_and_outcome() {
    let output_file = tempfile::NamedTempFile::new().expect("diagnostic output");
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(output_file.reopen().expect("writer")))
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let pair = SpaceAdmissionProtocolTestPair::authentication_rejected().await;
    pair.joiner()
        .start_join_at(join_input("private-device-name-sentinel"), 1_000)
        .await
        .expect("the join request should be saved before recovery");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.deferred_count, 1);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    let diagnostics = std::fs::read_to_string(output_file.path()).expect("diagnostics");
    for expected in [
        "pairing.recovery.decided",
        "state_changed",
        "authentication_rejected",
        "deferred",
    ] {
        assert!(
            diagnostics.contains(expected),
            "diagnostic output must contain {expected}: {diagnostics}"
        );
    }
    assert!(!diagnostics.contains("private-device-name-sentinel"));
}

#[tokio::test]
async fn short_code_resolution_is_marked_started_once_and_saves_the_full_invitation() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;
    pair.joiner()
        .start_join_at(join_input("short-once"), 1_000)
        .await
        .expect("the short code should be saved before resolution");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 2);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(pair.admission_status_invalidation_count(), 0);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedUnresolvedInvitation,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInvitationResolutionStarted,
            ProtocolEvent::JoinerInvitationResolutionRequested,
            ProtocolEvent::JoinerSavedResolvedInvitation,
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 1, "report: {report:?}");
    assert_eq!(report.deferred_count, 0);
    assert!(pair.take_created_join().invitation_resolution().is_none());
}

#[tokio::test]
async fn restarted_in_flight_short_code_is_rejected_without_a_second_resolution() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;
    pair.joiner()
        .start_join_at(join_input("short-once"), 1_000)
        .await
        .expect("the short code should be saved");
    pair.simulate_invitation_resolution_started();
    pair.clear_events();

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::Startup)
        .await;

    assert_eq!(report.rejected_count, 1);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    assert_eq!(pair.admission_status_invalidation_count(), 1);
    assert_eq!(
        pair.events(),
        &[ProtocolEvent::JoinerRejectedConsumedInvitation]
    );
    assert!(pair.take_created_join().is_terminal());
}

#[tokio::test]
async fn ambiguous_short_code_resolution_failure_is_rejected_without_retry() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;
    pair.joiner()
        .start_join_at(join_input("short-fail"), 1_000)
        .await
        .expect("the short code should be saved");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.rejected_count, 1);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    assert_eq!(pair.admission_status_invalidation_count(), 1);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedUnresolvedInvitation,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInvitationResolutionStarted,
            ProtocolEvent::JoinerInvitationResolutionRequested,
            ProtocolEvent::JoinerRejectedConsumedInvitation,
        ]
    );
}

#[tokio::test]
async fn initial_authentication_is_saved_before_the_original_join_request_is_exchanged() {
    let pair = SpaceAdmissionProtocolTestPair::authenticating().await;
    pair.joiner()
        .start_join_at(join_input("authenticated-join"), 1_000)
        .await
        .expect("the join request should be saved before recovery");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.deferred_count, 1);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedJoinRequest,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInitialChannelRequested,
            ProtocolEvent::JoinerAuthenticatedChannelSaved,
            ProtocolEvent::JoinerJoinRequestExchanged,
        ]
    );
}

#[tokio::test]
async fn authenticated_old_layout_is_persisted_as_an_explicit_upgrade_rejection() {
    let pair = SpaceAdmissionProtocolTestPair::peer_upgrade_required().await;
    pair.joiner()
        .start_join_at(join_input("upgrade-required"), 1_000)
        .await
        .expect("the join request should be saved before recovery");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.rejected_count, 1);
    assert_eq!(report.peer_upgrade_required_count, 1);
    assert_eq!(report.deferred_count, 0);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    assert_eq!(pair.admission_status_invalidation_count(), 1);
    assert_eq!(
        pair.take_created_join().rejection_reason(),
        Some(uc_core::membership::SpaceAdmissionRejectionReason::PeerUpgradeRequired)
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedJoinRequest,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInitialChannelRequested,
            ProtocolEvent::JoinerAuthenticatedChannelSaved,
            ProtocolEvent::JoinerJoinRequestExchanged,
            ProtocolEvent::JoinerRejectedPeerUpgrade,
        ]
    );
}

#[tokio::test]
async fn prepared_join_keeps_its_exact_request_until_the_upgraded_peer_recovers() {
    let pair = SpaceAdmissionProtocolTestPair::upgrade_once_on_prepared().await;
    pair.joiner()
        .start_join_at(join_input("upgrade-prepared"), 1_000)
        .await
        .expect("join request should be saved");
    let blocked = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(
        blocked.peer_upgrade_required_count,
        1,
        "blocked report: {blocked:?}, events: {:?}",
        pair.events()
    );
    assert_eq!(blocked.recovery_required_count, 0);
    assert_eq!(blocked.rejected_count, 0);
    let after_block = pair.admission_status_invalidation_count();
    assert_eq!(after_block, 1);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    let saved = pair.saved_join();
    assert!(saved.peer_upgrade_required());
    assert_eq!(
        saved
            .pending_exchange()
            .expect("Prepared request remains pending")
            .request_envelope()
            .kind(),
        uc_core::membership::SpaceAdmissionMessageKind::Prepared
    );

    pair.require_upgrade_once_more();
    let repeated = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::PeerOnline(
            uc_core::DeviceId::new("still-old-peer"),
        ))
        .await;
    assert_eq!(repeated.peer_upgrade_required_count, 1);
    assert_eq!(repeated.recovery_required_count, 0);
    assert_eq!(pair.active_joiner_observation_count(), 0);
    assert_eq!(pair.admission_status_invalidation_count(), after_block);

    let resumed = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::PeerOnline(
            uc_core::DeviceId::new("upgraded-peer"),
        ))
        .await;

    assert_eq!(resumed.peer_upgrade_required_count, 0);
    assert_eq!(resumed.recovery_required_count, 0);
    assert_eq!(pair.active_joiner_observation_count(), 1);
    assert_eq!(pair.admission_status_invalidation_count(), after_block + 1);
    assert!(!pair.saved_join().peer_upgrade_required());
}

#[tokio::test]
async fn applied_join_stays_pending_until_the_upgraded_peer_can_complete_it() {
    let pair = SpaceAdmissionProtocolTestPair::upgrade_once_on_applied().await;
    pair.joiner()
        .start_join_at(join_input("upgrade-applied"), 1_000)
        .await
        .expect("join request should be saved");
    let blocked = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(blocked.peer_upgrade_required_count, 1);
    assert_eq!(blocked.recovery_required_count, 0);
    let saved = pair.saved_join();
    assert!(saved.peer_upgrade_required());
    assert_eq!(
        saved
            .pending_exchange()
            .expect("Applied request remains pending")
            .request_envelope()
            .kind(),
        uc_core::membership::SpaceAdmissionMessageKind::Applied
    );

    let resumed = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::PeerOnline(
            uc_core::DeviceId::new("upgraded-peer"),
        ))
        .await;

    assert_eq!(resumed.peer_upgrade_required_count, 0);
    assert_eq!(resumed.recovery_required_count, 0);
    assert!(!pair.saved_join().peer_upgrade_required());
}

#[tokio::test]
async fn cancelling_join_ends_locally_without_waiting_for_peer_upgrade() {
    let pair = SpaceAdmissionProtocolTestPair::upgrade_once_on_cancel().await;
    let started = pair
        .joiner()
        .start_join_at(join_input("upgrade-cancel"), 1_000)
        .await
        .expect("join request should be saved");
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    let crate::space::admission::CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("new join should be pending");
    };
    pair.joiner()
        .cancel_join(join_id)
        .await
        .expect("prepared join should terminate locally");

    let blocked = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(blocked.peer_upgrade_required_count, 0);
    assert_eq!(blocked.recovery_required_count, 0);
    let saved = pair.saved_join();
    assert_eq!(
        saved.termination_reason(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Cancelled)
    );
    assert_eq!(
        saved
            .cleanup_obligation()
            .expect("Prepared Joiner keeps cleanup responsibility")
            .commit_knowledge(),
        uc_core::membership::AdmissionCommitKnowledge::Known
    );
    assert_eq!(pair.active_joiner_observation_count(), 0);
}

#[tokio::test]
async fn active_join_remains_active_while_final_settlement_waits_for_a_peer_upgrade() {
    let pair = SpaceAdmissionProtocolTestPair::upgrade_once_on_settlement().await;
    pair.joiner()
        .start_join_at(join_input("upgrade-settlement"), 1_000)
        .await
        .expect("join request should be saved");
    for _ in 0..3 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }
    pair.joiner()
        .complete_pending_space_transition()
        .await
        .expect("saved activation should complete");

    let blocked = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(blocked.peer_upgrade_required_count, 1);
    assert_eq!(blocked.recovery_required_count, 0);
    let saved = pair.saved_join();
    assert!(saved.is_active());
    assert!(saved.peer_upgrade_required());
    assert_eq!(
        saved
            .pending_exchange()
            .expect("CompleteAck remains pending")
            .request_envelope()
            .kind(),
        uc_core::membership::SpaceAdmissionMessageKind::CompleteAck
    );

    let resumed = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::PeerOnline(
            uc_core::DeviceId::new("upgraded-peer"),
        ))
        .await;

    assert_eq!(resumed.peer_upgrade_required_count, 0);
    assert_eq!(resumed.recovery_required_count, 0);
    let saved = pair.saved_join();
    assert!(saved.is_active_settled());
    assert!(!saved.peer_upgrade_required());
}

#[tokio::test]
async fn candidate_is_saved_without_scheduling_another_maintenance_wake() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    pair.joiner()
        .start_join_at(join_input("candidate-join"), 1_000)
        .await
        .expect("the join request should be saved before recovery");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 3);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedJoinRequest,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInitialChannelRequested,
            ProtocolEvent::JoinerAuthenticatedChannelSaved,
            ProtocolEvent::JoinerJoinRequestExchanged,
            ProtocolEvent::JoinerSavedCandidate,
            ProtocolEvent::JoinerSavedPrepared,
        ]
    );
}

#[tokio::test]
async fn candidate_and_commit_are_advanced_in_one_recovery() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    pair.joiner()
        .start_join_at(join_input("commit-join"), 1_000)
        .await
        .expect("the join request should be saved before recovery");
    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 5);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedJoinRequest,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInitialChannelRequested,
            ProtocolEvent::JoinerAuthenticatedChannelSaved,
            ProtocolEvent::JoinerJoinRequestExchanged,
            ProtocolEvent::JoinerSavedCandidate,
            ProtocolEvent::JoinerSavedPrepared,
            ProtocolEvent::JoinerContinuationChannelRequested,
            ProtocolEvent::JoinerPreparedExchanged,
            ProtocolEvent::JoinerSavedCommitted,
            ProtocolEvent::JoinerSavedApplied,
            ProtocolEvent::JoinerContinuationChannelRequested,
        ]
    );
}

#[tokio::test]
async fn maintenance_yields_after_the_activation_plan_is_saved() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join_at(join_input("apply-commit"), 1_000)
        .await
        .expect("the join request should be saved before recovery");
    let outcome = pair
        .joiner()
        .recover_space_admissions(&MembershipMaintenanceTrigger::StateChanged)
        .await;

    assert_eq!(outcome.step(), MembershipMaintenanceStepOutcome::Completed);
    assert!(!outcome.should_continue());
    assert!(pair
        .joiner()
        .has_pending_space_transition()
        .await
        .expect("the saved activation requires a lifecycle transition"));
}

#[tokio::test]
async fn complete_is_saved_as_an_activation_plan_before_local_activation() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join_at(join_input("complete-join"), 1_000)
        .await
        .expect("the join request should be saved before recovery");
    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 6);
    assert_eq!(report.recovery_required_count, 0);
    assert_eq!(
        report.disposition,
        AdmissionRecoveryDisposition::YieldMaintenance
    );
    assert!(pair.events().ends_with(&[
        ProtocolEvent::JoinerContinuationChannelRequested,
        ProtocolEvent::JoinerAppliedExchanged,
        ProtocolEvent::JoinerSavedActivating,
    ]));
}

#[tokio::test]
async fn saved_activation_waits_for_the_explicit_lifecycle_transition() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join_at(join_input("activate-complete"), 1_000)
        .await
        .expect("the join request should be saved before recovery");
    for _ in 0..3 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 0);
    assert_eq!(report.deferred_count, 0);
    assert!(!pair
        .events()
        .contains(&ProtocolEvent::JoinerActivationExecuted));
    assert!(pair
        .joiner()
        .has_pending_space_transition()
        .await
        .expect("the lifecycle transition should remain pending"));
}

#[tokio::test]
async fn activation_is_retried_from_the_saved_plan_after_commit_conflict() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join_at(join_input("retry-activation"), 1_000)
        .await
        .expect("the join request should be saved before recovery");
    for _ in 0..3 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }
    pair.fail_next_activation_commit();

    let conflicted = pair.joiner().complete_pending_space_transition().await;
    let recovered = pair.joiner().complete_pending_space_transition().await;

    assert!(conflicted.is_err());
    assert!(recovered.is_ok());
    assert!(pair.events().ends_with(&[
        ProtocolEvent::JoinerActivationExecuted,
        ProtocolEvent::JoinerActivationExecuted,
        ProtocolEvent::JoinerSavedActivePendingSettlement,
        ProtocolEvent::AdmissionRecoveryWoken,
    ]));
}

#[tokio::test]
async fn settled_is_saved_and_finishes_joiner_recovery() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join_at(join_input("settled-join"), 1_000)
        .await
        .expect("the join request should be saved before recovery");
    for _ in 0..4 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }

    pair.joiner()
        .complete_pending_space_transition()
        .await
        .expect("the Engine lifecycle should complete the saved transition");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 1);
    assert_eq!(report.recovery_required_count, 0);
    assert_eq!(
        report.disposition,
        AdmissionRecoveryDisposition::YieldMaintenance
    );
    assert!(pair.events().ends_with(&[
        ProtocolEvent::JoinerContinuationChannelRequested,
        ProtocolEvent::JoinerCompleteAckExchanged,
        ProtocolEvent::JoinerSavedActiveSettled,
        ProtocolEvent::AdmissionRecoveryWoken,
    ]));
    assert!(pair.take_created_join().is_active_settled());
    assert_eq!(pair.active_joiner_observation_count(), 0);
}

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("New device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
        preserve_unreadable_history: false,
    }
}
