use uc_core::membership::{
    AdmissionRecordPersistence, SpaceAdmissionMessageKind, SponsorAbandonmentCleanup,
    SponsorPairingConfirmationStatus,
};

use super::sponsor::HandleAuthenticatedSpaceAdmissionMessagePort;
use super::test_support::{
    authenticated_abandonment, authenticated_applied, authenticated_complete_ack,
    authenticated_join_request, authenticated_join_request_started_at, authenticated_prepared,
    authenticated_prepared_with_peers, ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::membership::{
    AdmissionMaintenanceOutcome, MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
    RecoverSpaceAdmissionsPort,
};

#[tokio::test]
async fn candidate_abandonment_is_saved_before_reply_and_duplicate_replays_it() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let candidate = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("JoinRequest should produce Candidate");
    let first = authenticated_abandonment(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
        None,
    );
    let duplicate = authenticated_abandonment(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
        None,
    );
    pair.seed_sponsor(candidate.into_admission());

    let abandoned = pair
        .sponsor()
        .handle(first)
        .await
        .expect("Abandonment should produce Abandoned");
    let expected_message_id = abandoned
        .envelope()
        .expect("Abandoned reply must be available")
        .header()
        .message_id();
    let admission = abandoned.into_admission();
    assert!(matches!(
        admission.abandonment_cleanup(),
        Some(SponsorAbandonmentCleanup::NotRequired)
    ));
    pair.seed_sponsor(admission);

    let replay = pair
        .sponsor()
        .handle(duplicate)
        .await
        .expect("duplicate Abandonment should replay Abandoned");
    assert_eq!(
        replay
            .envelope()
            .expect("replayed Abandoned must be available")
            .header()
            .message_id(),
        expected_message_id
    );
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::SponsorSavedAccepted,
            ProtocolEvent::SponsorSavedCandidate,
            ProtocolEvent::SponsorSavedAbandoned,
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
}

#[tokio::test]
async fn committed_abandonment_keeps_the_exact_member_revocation_target() {
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
    let abandonment = authenticated_abandonment(
        commit.envelope().expect("Commit reply must be available"),
        None,
    );
    pair.seed_sponsor(commit.into_admission());

    let abandoned = pair
        .sponsor()
        .handle(abandonment)
        .await
        .expect("Abandonment should produce Abandoned")
        .into_admission();

    assert!(matches!(
        abandoned.abandonment_cleanup(),
        Some(SponsorAbandonmentCleanup::Known(_))
    ));
    let persisted = abandoned
        .encode_persisted()
        .expect("abandoned state should persist");
    let reopened = uc_core::membership::SponsorAdmission::decode_persisted(&persisted)
        .expect("abandoned state should reopen");
    assert!(matches!(
        reopened.abandonment_cleanup(),
        Some(SponsorAbandonmentCleanup::Known(_))
    ));
}

#[tokio::test]
async fn late_abandonment_for_an_older_stage_cannot_end_the_advanced_attempt() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let candidate = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("JoinRequest should produce Candidate");
    let late_abandonment = authenticated_abandonment(
        candidate
            .envelope()
            .expect("Candidate reply must be available"),
        None,
    );
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
    pair.seed_sponsor(commit.into_admission());

    assert!(matches!(
        pair.sponsor().handle(late_abandonment).await,
        Err(super::HandleAuthenticatedSpaceAdmissionMessageError::Invalid { .. })
    ));
    assert!(!pair
        .events()
        .contains(&ProtocolEvent::SponsorSavedAbandoned));
}

#[tokio::test]
async fn applied_abandonment_without_claimed_binding_keeps_a_lookup_target() {
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
    let abandonment = authenticated_abandonment(
        complete
            .envelope()
            .expect("Complete reply must be available"),
        None,
    );
    pair.seed_sponsor(complete.into_admission());

    let abandoned = pair
        .sponsor()
        .handle(abandonment)
        .await
        .expect("Abandonment should produce Abandoned")
        .into_admission();

    assert!(matches!(
        abandoned.abandonment_cleanup(),
        Some(SponsorAbandonmentCleanup::Unknown { .. })
    ));
    pair.seed_sponsor(abandoned);

    let recovery = pair.recover_sponsor().await;

    assert_eq!(recovery.advanced_count, 1);
    assert_eq!(recovery.deferred_count, 0);
    assert_eq!(recovery.recovery_required_count, 0);
    assert!(pair.sponsor_abandonment_cleanup_complete());
    assert!(pair.events().ends_with(&[
        ProtocolEvent::SponsorMemberRevoked,
        ProtocolEvent::SponsorAbandonmentCleanupCompleted,
    ]));
}

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
async fn sponsor_rejects_a_future_attempt_before_consuming_the_invitation() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;

    assert!(matches!(
        pair.sponsor()
            .handle(authenticated_join_request_started_at(2_000))
            .await,
        Err(super::HandleAuthenticatedSpaceAdmissionMessageError::Invalid { .. })
    ));
    assert!(pair.events().is_empty());
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
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
}

#[tokio::test]
async fn sponsor_keeps_the_member_unconfirmed_and_accepts_a_late_complete_ack() {
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
    let late_ack = authenticated_complete_ack(
        complete
            .envelope()
            .expect("Complete reply must be available"),
    );
    pair.seed_sponsor(complete.into_admission());

    pair.set_now_ms(301_000);
    let report = pair.recover_sponsor().await;

    assert_eq!(report.advanced_count, 1);
    assert_eq!(
        pair.sponsor_confirmation_status(),
        Some(SponsorPairingConfirmationStatus::Unconfirmed)
    );

    let settled = pair
        .sponsor()
        .handle(late_ack)
        .await
        .expect("late CompleteAck should still settle the exact admission");
    pair.seed_sponsor(settled.into_admission());
    assert_eq!(
        pair.sponsor_confirmation_status(),
        Some(SponsorPairingConfirmationStatus::Confirmed)
    );
}

#[tokio::test]
async fn sponsor_unfinished_attempt_ends_itself_at_the_shared_deadline() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    let candidate = pair
        .sponsor()
        .handle(authenticated_join_request())
        .await
        .expect("JoinRequest should produce Candidate");
    pair.seed_sponsor(candidate.into_admission());

    pair.set_now_ms(301_000);
    let report = pair.recover_sponsor().await;

    assert_eq!(report.advanced_count, 1);
    assert!(pair.sponsor_is_terminal());
    assert!(pair.sponsor_abandonment_cleanup_complete());
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
            ProtocolEvent::AdmissionRecoveryWoken,
            ProtocolEvent::AdmissionRecoveryWoken,
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
    pair.seed_sponsor(complete.into_admission());
    assert_eq!(
        pair.sponsor()
            .recover_space_admissions(&MembershipMaintenanceTrigger::StateChanged)
            .await,
        AdmissionMaintenanceOutcome::Yield(MembershipMaintenanceStepOutcome::Completed)
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
