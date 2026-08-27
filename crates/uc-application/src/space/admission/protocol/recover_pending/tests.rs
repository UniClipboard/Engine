use super::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    LoadedPendingAdmission,
};
use crate::space::admission::protocol::test_support::{
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::JoinSpaceInput;

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

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("New device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
        preserve_unreadable_history: false,
    }
}
