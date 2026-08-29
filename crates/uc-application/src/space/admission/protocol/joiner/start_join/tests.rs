use super::ports::{JoinerStartMaterialError, JoinerStartStateError};
use super::{LoadedJoinerStartState, SpaceAdmissionCommitToken};
use crate::space::admission::protocol::test_support::{
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::{CurrentJoinStatus, JoinSpaceError, JoinSpaceInput};
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
        JoinSpaceError::from(JoinerStartMaterialError::Unavailable),
        JoinSpaceError::Unavailable
    ));
}

#[tokio::test]
async fn fresh_join_is_saved_before_pending_is_returned() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;

    let started = pair
        .joiner()
        .start_join(JoinSpaceInput {
            invitation_code: uc_core::pairing::InvitationCode::new("fresh-join"),
            device_name: Some("New device".to_owned()),
            passphrase: uc_core::crypto::domain::Passphrase::new("correct horse battery staple"),
            preserve_unreadable_history: false,
        })
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
async fn short_code_is_saved_before_any_resolution_or_start_material() {
    let pair = SpaceAdmissionProtocolTestPair::short_invitation().await;

    let started = pair
        .joiner()
        .start_join(join_input("short-once"))
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
        .start_join(join_input("first-join"))
        .await
        .expect("the first join should be saved");
    let current_join = first.take_created_join();

    let replacement = SpaceAdmissionProtocolTestPair::with_current_join(Some(current_join)).await;
    replacement
        .joiner()
        .start_join(join_input("replacement-join"))
        .await
        .expect("an Initiated join can be superseded");

    assert!(replacement.superseded_previous_join());
}

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("New device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
        preserve_unreadable_history: false,
    }
}
