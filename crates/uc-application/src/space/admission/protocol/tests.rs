use uc_core::membership::SpaceAdmissionMessageKind;

use super::sponsor::HandleAuthenticatedSpaceAdmissionMessagePort;
use super::test_support::{
    authenticated_join_request, authenticated_prepared, authenticated_prepared_with_peers,
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};

#[tokio::test]
async fn prepared_is_committed_before_the_sponsor_returns_commit() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let candidate = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("JoinRequest should produce Candidate");
    let prepared = authenticated_prepared(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
    );
    pair.seed_sponsor(candidate.into_aggregate());

    let commit = pair
        .sponsor()
        .handle(prepared)
        .await
        .expect("Prepared should produce Commit");

    assert_eq!(
        commit
            .envelope()
            .expect("Commit reply must be available")
            .kind(),
        SpaceAdmissionMessageKind::Commit
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
            ProtocolEvent::SponsorSavedCommitted,
        ]
    );
}

#[tokio::test]
async fn prepared_from_a_different_authenticated_peer_is_rejected_before_commit() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let candidate = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("JoinRequest should produce Candidate");
    let prepared = authenticated_prepared_with_peers(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
        0xa3,
        0xa4,
    );
    pair.seed_sponsor(candidate.into_aggregate());

    assert!(matches!(
        pair.sponsor().handle(prepared).await,
        Err(super::HandleAuthenticatedSpaceAdmissionMessageError::Conflict { .. })
    ));
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
        ]
    );
}

#[tokio::test]
async fn duplicate_prepared_replays_the_saved_commit_without_a_new_commit() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let candidate = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("JoinRequest should produce Candidate");
    let first_prepared = authenticated_prepared(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
    );
    let duplicate_prepared = authenticated_prepared(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
    );
    pair.seed_sponsor(candidate.into_aggregate());
    let committed = pair
        .sponsor()
        .handle(first_prepared)
        .await
        .expect("Prepared should produce Commit");
    let expected_message_id = committed
        .envelope()
        .expect("Commit reply must be available")
        .header()
        .message_id();
    pair.seed_sponsor(committed.into_aggregate());

    let replay = pair
        .sponsor()
        .handle(duplicate_prepared)
        .await
        .expect("duplicate Prepared should replay Commit");

    assert_eq!(
        replay
            .envelope()
            .expect("replayed Commit must be available")
            .header()
            .message_id(),
        expected_message_id
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
            ProtocolEvent::SponsorSavedCommitted,
        ]
    );
}
