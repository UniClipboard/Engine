use super::ports::{JoinerStartMaterialError, JoinerStartStateError};
use super::{LoadedJoinerStartState, SpaceAdmissionCommitToken};
use crate::space::admission::protocol::test_support::{
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::{
    AdmissionRecoveryTrigger, CurrentJoinStatus, JoinSpaceError, JoinSpaceInput,
};
use uc_core::membership::AdmissionSourceSnapshot;

#[test]
fn admission_commit_token_rejects_zero_and_redacts_its_value() {
    assert!(SpaceAdmissionCommitToken::from_bytes([0; 32]).is_none());

    let token = SpaceAdmissionCommitToken::from_bytes([0x21; 32])
        .expect("a non-zero commit token is valid");

    assert_eq!(token.as_bytes(), &[0x21; 32]);
    assert_eq!(
        format!("{token:?}"),
        "SpaceAdmissionCommitToken([REDACTED])"
    );
}

#[test]
fn loaded_joiner_start_state_keeps_every_fact_needed_for_one_start() {
    let source_snapshot =
        AdmissionSourceSnapshot::from_bytes(vec![0x22; 32]).expect("valid source snapshot");
    let commit_token =
        SpaceAdmissionCommitToken::from_bytes([0x23; 32]).expect("valid commit token");
    let loaded = LoadedJoinerStartState::new(7, source_snapshot, None, true, commit_token);

    let (
        next_local_join_ordinal,
        source_snapshot,
        current_join,
        requires_session_transition,
        commit_token,
    ) = loaded.into_parts();

    assert_eq!(next_local_join_ordinal, 7);
    assert_eq!(source_snapshot.as_bytes(), &[0x22; 32]);
    assert!(current_join.is_none());
    assert!(requires_session_transition);
    assert_eq!(commit_token.as_bytes(), &[0x23; 32]);
}

#[test]
fn joiner_start_state_errors_keep_distinct_join_space_categories() {
    assert!(matches!(
        JoinSpaceError::from(JoinerStartStateError::Locked),
        JoinSpaceError::Locked
    ));
    assert!(matches!(
        JoinSpaceError::from(JoinerStartStateError::StateChanged),
        JoinSpaceError::StateChanged
    ));
    assert!(matches!(
        JoinSpaceError::from(JoinerStartStateError::RecoveryRequired),
        JoinSpaceError::RecoveryRequired
    ));
    assert!(matches!(
        JoinSpaceError::from(JoinerStartStateError::Unavailable),
        JoinSpaceError::Unavailable
    ));
}

#[test]
fn joiner_start_material_errors_keep_distinct_join_space_categories() {
    assert!(matches!(
        JoinSpaceError::from(JoinerStartMaterialError::InvalidInvitation),
        JoinSpaceError::InvalidInvitation
    ));
    assert!(matches!(
        JoinSpaceError::from(JoinerStartMaterialError::unavailable(anyhow::anyhow!(
            "fixture"
        ))),
        JoinSpaceError::Unavailable
    ));
}

#[test]
fn unavailable_start_material_error_preserves_its_source() {
    use std::error::Error;

    let error = JoinerStartMaterialError::unavailable(anyhow::anyhow!("fixture"));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn fresh_join_is_saved_before_pending_is_returned() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;

    let started = pair
        .joiner()
        .start_join_at(
            JoinSpaceInput {
                invitation_code: uc_core::pairing::InvitationCode::new("fresh-join"),
                device_name: Some("New device".to_owned()),
                passphrase: uc_core::crypto::domain::Passphrase::new(
                    "correct horse battery staple",
                ),
                preserve_unreadable_history: false,
            },
            1_000,
        )
        .await
        .expect("a fresh join should be saved locally");

    assert!(matches!(started.status, CurrentJoinStatus::Pending { .. }));
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedJoinRequest,
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
}

#[tokio::test]
async fn a_join_that_spends_its_budget_before_commit_is_saved_as_expired() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;
    pair.set_now_ms(301_000);

    let started = pair
        .joiner()
        .start_join_at(join_input("expired-before-commit"), 1_000)
        .await
        .expect("expired result is saved");

    assert!(matches!(
        started.status,
        CurrentJoinStatus::Terminated {
            reason: crate::space::admission::JoinSpaceTerminationReason::Expired,
            ..
        }
    ));
    assert_eq!(
        pair.take_created_join().termination_reason(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Expired)
    );
}

#[tokio::test]
async fn short_code_is_saved_before_any_resolution_or_start_material() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;

    let started = pair
        .joiner()
        .start_join_at(join_input("short-once"), 1_000)
        .await
        .expect("the unresolved short code should be saved");

    assert!(matches!(started.status, CurrentJoinStatus::Pending { .. }));
    assert_eq!(
        pair.events(),
        &[
            ProtocolEvent::DeviceNameSaved,
            ProtocolEvent::JoinerSavedUnresolvedInvitation,
            ProtocolEvent::AdmissionRecoveryWoken,
        ]
    );
    assert!(matches!(
        pair.take_created_join().invitation_resolution(),
        Some(uc_core::membership::JoinerInvitationResolution::Ready { short_code, .. })
            if short_code.as_bytes() == b"short-once"
    ));
}

#[tokio::test]
async fn a_replaceable_current_join_is_superseded_with_the_new_join_in_one_commit() {
    let first = SpaceAdmissionProtocolTestPair::fresh().await;
    first
        .joiner()
        .start_join_at(join_input("first-join"), 1_000)
        .await
        .expect("the first join should be saved");
    let current_join = first.take_created_join();
    let current_observation_material = *current_join.admission_id().as_bytes();

    let replacement = SpaceAdmissionProtocolTestPair::with_current_join(Some(current_join)).await;
    replacement.begin_joiner_observation(current_observation_material);
    assert_eq!(replacement.active_joiner_observation_count(), 1);
    replacement
        .joiner()
        .start_join_at(join_input("replacement-join"), 1_000)
        .await
        .expect("an Initiated join can be superseded");

    assert!(replacement.superseded_previous_join());
    assert_eq!(replacement.active_joiner_observation_count(), 1);
}

#[tokio::test]
async fn a_committed_current_join_does_not_block_a_distinct_new_join() {
    let first = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    first
        .joiner()
        .start_join_at(join_input("committed-attempt-a"), 1_000)
        .await
        .expect("attempt A is saved");
    first
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    first
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    let attempt_a = first.take_created_join();

    let replacement = SpaceAdmissionProtocolTestPair::with_current_join(Some(attempt_a)).await;
    let attempt_b = replacement
        .joiner()
        .start_join_at(join_input("attempt-b"), 2_000)
        .await
        .expect("attempt B replaces committed attempt A");

    assert!(matches!(
        attempt_b.status,
        CurrentJoinStatus::Pending { .. }
    ));
    assert!(replacement.superseded_previous_join());
    assert_eq!(
        replacement.previous_termination(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Superseded)
    );
}

#[tokio::test]
async fn an_expired_join_is_ended_when_a_distinct_new_join_is_saved() {
    let first = SpaceAdmissionProtocolTestPair::fresh().await;
    first
        .joiner()
        .start_join_at(join_input("attempt-a"), 1_000)
        .await
        .expect("attempt A is saved");
    let attempt_a = first.take_created_join();

    let replacement = SpaceAdmissionProtocolTestPair::with_current_join(Some(attempt_a)).await;
    let attempt_b = replacement
        .joiner()
        .start_join_at(join_input("attempt-b"), 301_000)
        .await
        .expect("attempt B replaces expired A");

    assert!(matches!(
        attempt_b.status,
        CurrentJoinStatus::Pending { .. }
    ));
    assert_eq!(
        replacement.previous_termination(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Expired)
    );
    assert_eq!(
        replacement.take_created_join().expires_at_ms(),
        Some(601_000)
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
