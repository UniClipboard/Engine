use uc_core::membership::SpaceAdmissionMessageKind;

use super::sponsor::HandleAuthenticatedSpaceAdmissionMessagePort;
use super::test_support::{
    authenticated_applied, authenticated_complete_ack, authenticated_join_request,
    authenticated_prepared, authenticated_prepared_with_peers, ProtocolEvent,
    SpaceAdmissionProtocolTestPair,
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
    pair.seed_sponsor(candidate.into_admission());

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
async fn complete_ack_is_saved_before_the_sponsor_returns_settled() {
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
    pair.seed_sponsor(candidate.into_admission());
    let commit = pair
        .sponsor()
        .handle(prepared)
        .await
        .expect("Prepared should produce Commit");
    let applied = authenticated_applied(commit.envelope().expect("Commit reply must be available"));
    pair.seed_sponsor(commit.into_admission());
    let complete = pair
        .sponsor()
        .handle(applied)
        .await
        .expect("Applied should produce Complete");
    let complete_ack = authenticated_complete_ack(
        complete
            .envelope()
            .expect("Complete reply must be available"),
    );
    pair.seed_sponsor(complete.into_admission());

    let settled = pair
        .sponsor()
        .handle(complete_ack)
        .await
        .expect("CompleteAck should produce Settled");

    assert_eq!(
        settled
            .envelope()
            .expect("Settled reply must be available")
            .kind(),
        SpaceAdmissionMessageKind::Settled
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
            ProtocolEvent::SponsorSavedCommitted,
            ProtocolEvent::SponsorSavedApplied,
            ProtocolEvent::SponsorSavedCompleted,
        ]
    );
}

#[tokio::test]
async fn duplicate_complete_ack_replays_settled_without_a_new_commit() {
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
    pair.seed_sponsor(candidate.into_admission());
    let commit = pair
        .sponsor()
        .handle(prepared)
        .await
        .expect("Prepared should produce Commit");
    let applied = authenticated_applied(commit.envelope().expect("Commit reply must be available"));
    pair.seed_sponsor(commit.into_admission());
    let complete = pair
        .sponsor()
        .handle(applied)
        .await
        .expect("Applied should produce Complete");
    let first_complete_ack = authenticated_complete_ack(
        complete
            .envelope()
            .expect("Complete reply must be available"),
    );
    let duplicate_complete_ack = authenticated_complete_ack(
        complete
            .envelope()
            .expect("Complete reply must be available"),
    );
    pair.seed_sponsor(complete.into_admission());
    let settled = pair
        .sponsor()
        .handle(first_complete_ack)
        .await
        .expect("CompleteAck should produce Settled");
    let expected_message_id = settled
        .envelope()
        .expect("Settled reply must be available")
        .header()
        .message_id();
    pair.seed_sponsor(settled.into_admission());

    let replay = pair
        .sponsor()
        .handle(duplicate_complete_ack)
        .await
        .expect("duplicate CompleteAck should replay Settled");

    assert_eq!(
        replay
            .envelope()
            .expect("replayed Settled must be available")
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
            ProtocolEvent::SponsorSavedApplied,
            ProtocolEvent::SponsorSavedCompleted,
        ]
    );
}

#[tokio::test]
async fn applied_is_saved_before_the_sponsor_returns_complete() {
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
    pair.seed_sponsor(candidate.into_admission());
    let commit = pair
        .sponsor()
        .handle(prepared)
        .await
        .expect("Prepared should produce Commit");
    let applied = authenticated_applied(commit.envelope().expect("Commit reply must be available"));
    pair.seed_sponsor(commit.into_admission());

    let complete = pair
        .sponsor()
        .handle(applied)
        .await
        .expect("Applied should produce Complete");

    assert_eq!(
        complete
            .envelope()
            .expect("Complete reply must be available")
            .kind(),
        SpaceAdmissionMessageKind::Complete
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
    pair.seed_sponsor(candidate.into_admission());

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
    pair.seed_sponsor(candidate.into_admission());
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
    pair.seed_sponsor(committed.into_admission());

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
