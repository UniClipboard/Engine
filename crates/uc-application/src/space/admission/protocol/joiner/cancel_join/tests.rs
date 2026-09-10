use crate::space::admission::protocol::test_support::SpaceAdmissionProtocolTestPair;
use crate::space::admission::{
    AdmissionRecoveryTrigger, CancelSpaceJoinError, CurrentJoinStatus, JoinSpaceInput,
};
use uc_core::membership::{AdmissionPendingRecovery, SpaceAdmissionMessageKind};

#[tokio::test]
async fn current_prepared_join_is_replaced_by_one_saved_cancel_request() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    let started = pair
        .joiner()
        .start_join(join_input("cancel-prepared"))
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

    assert!(matches!(
        status,
        CurrentJoinStatus::Pending {
            cancel_requested: true,
            ..
        }
    ));
    let cancelled = pair.take_created_join();
    let Some(AdmissionPendingRecovery::Continuation {
        pending_exchange, ..
    }) = cancelled.pending_recovery()
    else {
        panic!("saved cancellation should be recoverable");
    };
    assert_eq!(
        pending_exchange.request_envelope().kind(),
        SpaceAdmissionMessageKind::CancelRequested
    );
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
        .start_join(join_input("cancel-conflict"))
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
    assert!(matches!(
        status,
        CurrentJoinStatus::Pending {
            cancel_requested: true,
            ..
        }
    ));
    assert_eq!(
        pair.take_created_join().rejection_reason(),
        Some(uc_core::membership::SpaceAdmissionRejectionReason::Cancelled)
    );
}

#[tokio::test]
async fn persistent_cancellation_conflicts_preserve_the_failure_source() {
    let pair = SpaceAdmissionProtocolTestPair::receiving_candidate().await;
    let started = pair
        .joiner()
        .start_join(join_input("cancel-conflicts"))
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
