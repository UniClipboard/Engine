use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    LoadedPendingAdmission,
};
use crate::space::admission::protocol::test_support::{
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::JoinSpaceInput;
use uc_core::membership::AdmissionRecordPersistence;

#[tokio::test]
async fn loaded_pending_admission_keeps_the_aggregate_and_commit_token_together() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    pair.joiner()
        .start_join(join_input("loaded-recovery"))
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
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    pair.joiner()
        .start_join(join_input("recoverable-join"))
        .await
        .expect("the join request should be saved before recovery");

    let report: AdmissionRecoveryReport = pair
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
        ]
    );
}

#[tokio::test]
async fn short_code_resolution_is_marked_started_once_and_saves_the_full_invitation() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;
    pair.joiner()
        .start_join(join_input("short-once"))
        .await
        .expect("the short code should be saved before resolution");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 2);
    assert_eq!(report.deferred_count, 1);
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedUnresolvedInvitation,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerInvitationResolutionStarted,
            ProtocolEvent::JoinerInvitationResolutionRequested,
            ProtocolEvent::JoinerSavedResolvedInvitation,
        ]
    );
    assert!(matches!(
        pair.take_created_join().invitation_resolution(),
        Some(uc_core::membership::JoinerInvitationResolution::Resolved {
            full_invitation,
            ..
        }) if full_invitation.as_str() == "ucspace1_resolved-short-once"
    ));
}

#[tokio::test]
async fn restarted_in_flight_short_code_is_rejected_without_a_second_resolution() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;
    pair.joiner()
        .start_join(join_input("short-once"))
        .await
        .expect("the short code should be saved");
    pair.simulate_invitation_resolution_started();
    pair.clear_events();

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::Startup)
        .await;

    assert_eq!(report.rejected_count, 1);
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
        .start_join(join_input("short-fail"))
        .await
        .expect("the short code should be saved");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.rejected_count, 1);
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
        .start_join(join_input("authenticated-join"))
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
async fn candidate_is_saved_then_prepared_before_the_next_exchange_is_woken() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    pair.joiner()
        .start_join(join_input("candidate-join"))
        .await
        .expect("the join request should be saved before recovery");

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 3);
    assert_eq!(report.deferred_count, 0);
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
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
}

#[tokio::test]
async fn prepared_is_exchanged_and_commit_is_saved_on_the_next_recovery() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    pair.joiner()
        .start_join(join_input("commit-join"))
        .await
        .expect("the join request should be saved before recovery");
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 2);
    assert_eq!(report.deferred_count, 0);
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
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerContinuationChannelRequested,
            ProtocolEvent::JoinerPreparedExchanged,
            ProtocolEvent::JoinerSavedCommitted,
            ProtocolEvent::JoinerSavedApplied,
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
}

#[tokio::test]
async fn commit_is_applied_and_saved_before_the_next_exchange_is_woken() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    pair.joiner()
        .start_join(join_input("apply-commit"))
        .await
        .expect("the join request should be saved before recovery");
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 2);
    assert_eq!(report.deferred_count, 0);
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
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::JoinerContinuationChannelRequested,
            ProtocolEvent::JoinerPreparedExchanged,
            ProtocolEvent::JoinerSavedCommitted,
            ProtocolEvent::JoinerSavedApplied,
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
}

#[tokio::test]
async fn complete_is_saved_as_an_activation_plan_before_local_activation() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join(join_input("complete-join"))
        .await
        .expect("the join request should be saved before recovery");
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 1);
    assert_eq!(report.recovery_required_count, 0);
    assert!(pair.events().ends_with(&[
        ProtocolEvent::JoinerContinuationChannelRequested,
        ProtocolEvent::JoinerAppliedExchanged,
        ProtocolEvent::JoinerSavedActivating,
    ]));
}

#[tokio::test]
async fn saved_activation_is_executed_before_complete_ack_is_woken() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join(join_input("activate-complete"))
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

    assert_eq!(report.advanced_count, 1);
    assert_eq!(report.deferred_count, 0);
    assert!(pair.events().ends_with(&[
        ProtocolEvent::JoinerActivationExecuted,
        ProtocolEvent::JoinerSavedActivePendingSettlement,
        ProtocolEvent::AdmissionRecoveryWoken,
    ]));
}

#[tokio::test]
async fn activation_is_retried_from_the_saved_plan_after_commit_conflict() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join(join_input("retry-activation"))
        .await
        .expect("the join request should be saved before recovery");
    for _ in 0..3 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }
    pair.fail_next_activation_commit();

    let conflicted = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    let recovered = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(conflicted.deferred_count, 1);
    assert_eq!(conflicted.advanced_count, 0);
    assert_eq!(recovered.advanced_count, 1);
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
        .start_join(join_input("settled-join"))
        .await
        .expect("the join request should be saved before recovery");
    for _ in 0..4 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }

    let report = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;

    assert_eq!(report.advanced_count, 1);
    assert_eq!(report.recovery_required_count, 0);
    assert!(pair.events().ends_with(&[
        ProtocolEvent::JoinerContinuationChannelRequested,
        ProtocolEvent::JoinerCompleteAckExchanged,
        ProtocolEvent::JoinerSavedActiveSettled,
    ]));
    assert!(pair.take_created_join().is_active_settled());
}

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("New device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
        preserve_unreadable_history: false,
    }
}
