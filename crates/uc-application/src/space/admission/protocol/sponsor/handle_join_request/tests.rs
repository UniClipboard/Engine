use uc_core::membership::SpaceAdmissionMessageKind;

use crate::space::admission::protocol::test_support::{
    authenticated_join_request, ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::protocol::HandleAuthenticatedSpaceAdmissionMessagePort;

#[tokio::test]
async fn sponsor_saves_accepted_then_candidate_before_returning_the_reply() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;

    let reply = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("a fresh authenticated JoinRequest should produce Candidate");

    assert_eq!(
        reply
            .envelope()
            .expect("Candidate reply must be available")
            .kind(),
        SpaceAdmissionMessageKind::Candidate
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
        ]
    );
}

#[tokio::test]
async fn duplicate_join_request_returns_the_saved_candidate_without_new_commits() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let first = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("the first JoinRequest should produce Candidate");
    let first_message_id = first
        .envelope()
        .expect("Candidate reply must be available")
        .header()
        .message_id();
    pair.seed_sponsor(first.into_admission());

    let replay = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("the duplicate JoinRequest should replay Candidate");

    assert_eq!(
        replay
            .envelope()
            .expect("replayed Candidate must be available")
            .header()
            .message_id(),
        first_message_id
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
        ]
    );
}
