use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinError;
use tokio::time::timeout;

use super::WorkOwner;

#[tokio::test]
async fn shutdown_waits_for_both_the_accepted_action_and_its_continuation() {
    let owner = WorkOwner::default();
    let action = owner.begin().unwrap();
    let continuation = action.continuation();
    assert!(timeout(Duration::ZERO, owner.shutdown()).await.is_err());
    assert!(owner.begin().is_none());
    drop(action);
    assert!(timeout(Duration::ZERO, owner.shutdown()).await.is_err());
    drop(continuation);
    owner.shutdown().await.unwrap();
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn background_failure_is_retained_without_skipping_other_accepted_work() {
    let owner = WorkOwner::default();
    let first = owner.begin().unwrap();
    let second = owner.begin().unwrap();
    let failure = tokio::spawn(async { panic!("private delivery failure") })
        .await
        .unwrap_err();
    first.failed(failure);
    drop(first);
    assert!(timeout(Duration::ZERO, owner.shutdown()).await.is_err());
    drop(second);
    for _ in 0..2 {
        let error = owner.shutdown().await.unwrap_err();
        assert!(error
            .primary
            .downcast_ref::<Arc<JoinError>>()
            .unwrap()
            .is_panic());
        assert!(!format!("{error:?}").contains("private"));
    }
}
