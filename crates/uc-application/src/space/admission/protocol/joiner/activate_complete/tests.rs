use crate::space::admission::protocol::test_support::SpaceAdmissionProtocolTestPair;
use crate::space::admission::{AdmissionRecoveryTrigger, CurrentJoinStatus, JoinSpaceInput};

#[tokio::test]
async fn protocol_queries_and_completes_the_saved_activation_as_one_action() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    pair.joiner()
        .start_join(join_input("explicit-activation"))
        .await
        .expect("join should be saved");
    for _ in 0..3 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }

    assert!(pair
        .joiner()
        .has_pending_space_transition()
        .await
        .expect("pending activation query should succeed"));
    let status = pair
        .joiner()
        .complete_pending_space_transition()
        .await
        .expect("saved activation should complete");

    assert!(matches!(
        status,
        CurrentJoinStatus::Active {
            joined_space,
            ..
        } if joined_space.space_id == "target-space"
    ));
    assert!(!pair
        .joiner()
        .has_pending_space_transition()
        .await
        .expect("completed activation should no longer be pending"));
}

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("New device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
        preserve_unreadable_history: false,
    }
}
