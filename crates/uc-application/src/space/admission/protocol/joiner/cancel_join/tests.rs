use crate::space::admission::protocol::test_support::SpaceAdmissionProtocolTestPair;
use crate::space::admission::{
    AdmissionRecoveryTrigger, CancelSpaceJoinError, CurrentJoinStatus, JoinSpaceInput,
};
use uc_core::membership::AdmissionCommitKnowledge;

#[tokio::test]
async fn current_prepared_join_terminates_locally_with_unknown_cleanup() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    let started = pair
        .joiner()
        .start_join_at(join_input("cancel-prepared"), 1_000)
        .await
        .expect("join should be saved");
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    let CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("new join should be pending");
    };

    let status = pair
        .joiner()
        .cancel_join(join_id)
        .await
        .expect("prepared join should accept cancellation");

    assert!(matches!(status, CurrentJoinStatus::Terminated { .. }));
    let cancelled = pair.take_created_join();
    assert_eq!(
        cancelled
            .cleanup_obligation()
            .expect("Prepared Joiner keeps cleanup responsibility")
            .commit_knowledge(),
        AdmissionCommitKnowledge::Unknown
    );
}

#[tokio::test]
async fn current_committed_join_terminates_locally_with_exact_cleanup_target() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    let started = pair
        .joiner()
        .start_join_at(join_input("cancel-committed"), 1_000)
        .await
        .expect("join should be saved");
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    pair.joiner()
        .recover_pending(AdmissionRecoveryTrigger::StateChanged)
        .await;
    let CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("new join should be pending");
    };

    let status = pair
        .joiner()
        .cancel_join(join_id)
        .await
        .expect("committed join should terminate locally");

    assert!(matches!(status, CurrentJoinStatus::Terminated { .. }));
    let cancelled = pair.take_created_join();
    let cleanup = cancelled
        .cleanup_obligation()
        .expect("Committed Joiner keeps cleanup responsibility");
    assert_eq!(cleanup.commit_knowledge(), AdmissionCommitKnowledge::Known);
    assert!(cleanup.member_binding().is_some());
}

#[tokio::test]
async fn activating_join_is_isolated_before_cancellation_returns() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_complete().await;
    let started = pair
        .joiner()
        .start_join_at(join_input("cancel-activating"), 1_000)
        .await
        .expect("join should be saved");
    for _ in 0..3 {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }
    let CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("new join should be pending");
    };

    let status = pair
        .joiner()
        .cancel_join(join_id)
        .await
        .expect("activating join should terminate locally");

    assert!(matches!(status, CurrentJoinStatus::Terminated { .. }));
    assert!(pair
        .events()
        .contains(&super::super::super::test_support::ProtocolEvent::JoinerActivationTerminated));
    let cancelled = pair.take_created_join();
    assert!(cancelled
        .cleanup_obligation()
        .and_then(uc_core::membership::AdmissionCleanupObligation::local_space_transition)
        .is_some());
}

#[tokio::test]
async fn cancellation_only_targets_the_current_join_id() {
    let pair = SpaceAdmissionProtocolTestPair::fresh().await;

    assert!(matches!(
        pair.joiner().cancel_join([0x7f; 16]).await,
        Err(CancelSpaceJoinError::NotFound)
    ));
}

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("New device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
        preserve_unreadable_history: false,
    }
}

#[tokio::test]
async fn cancellation_reloads_after_a_concurrent_admission_update() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    let started = pair
        .joiner()
        .start_join_at(join_input("cancel-conflict"), 1_000)
        .await
        .expect("saved join");
    let CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("pending join");
    };
    pair.inject_cancellation_conflicts(1);
    let status = pair
        .joiner()
        .cancel_join(join_id)
        .await
        .expect("cancellation survives concurrent update");
    assert!(matches!(status, CurrentJoinStatus::Terminated { .. }));
    assert_eq!(
        pair.take_created_join().termination_reason(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Cancelled)
    );
}

#[tokio::test]
async fn persistent_cancellation_conflicts_preserve_the_failure_source() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    let started = pair
        .joiner()
        .start_join_at(join_input("cancel-conflicts"), 1_000)
        .await
        .expect("saved join");
    let CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("pending join");
    };
    pair.inject_cancellation_conflicts(10);
    let error = pair
        .joiner()
        .cancel_join(join_id)
        .await
        .expect_err("persistent conflict must stop");
    let CancelSpaceJoinError::State { source } = error else {
        panic!("state error expected");
    };
    assert!(matches!(
        source.downcast_ref::<super::JoinerCancellationStateError>(),
        Some(super::JoinerCancellationStateError::StateChanged { .. })
    ));
    assert!(source.chain().count() >= 2);
    assert!(!pair.take_created_join().is_cancelling());
}
