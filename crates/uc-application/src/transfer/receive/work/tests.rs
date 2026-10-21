use super::{ReceiveWorkError, ReceiveWorkRegistry};
use std::io;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use uc_core::ids::EntryId;

#[tokio::test]
async fn cancelling_an_attempt_seals_late_children_and_waits_for_the_whole_action() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let a = registry.action();
    let b = registry.action();
    let entry = EntryId::from("entry");
    a.register(&entry, "attempt-a");
    b.register(&entry, "attempt-b");
    let stopped = registry.stop(&entry, "attempt-a").unwrap();
    assert!(a.cancellation().child_token().is_cancelled());
    assert!(!b.cancellation().is_cancelled());
    let mut waiting = Box::pin(stopped.wait());
    assert!(timeout(Duration::from_millis(10), waiting.as_mut())
        .await
        .is_err());
    a.finish(Ok(()));
    waiting.await.unwrap();
    b.finish(Ok(()));
}

#[tokio::test]
async fn settlement_failure_keeps_its_source_and_does_not_become_successful_stop() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    let entry = EntryId::from("entry");
    action.register(&entry, "attempt");
    let stopped = registry.stop(&entry, "attempt").unwrap();
    action.finish(Err(ReceiveWorkError::Action(
        io::Error::from(io::ErrorKind::PermissionDenied).into(),
    )));
    let error = stopped.wait().await.unwrap_err();
    let ReceiveWorkError::Action(source) = error.as_ref() else {
        panic!("typed action source")
    };
    assert_eq!(
        source.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[tokio::test]
async fn task_panic_is_retained_by_the_same_completion() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let action = registry.action();
    let entry = EntryId::from("entry");
    action.register(&entry, "attempt");
    let stopped = registry.stop(&entry, "attempt").unwrap();
    let source = Arc::new(
        tokio::spawn(async { panic!("injected receive panic") })
            .await
            .unwrap_err(),
    );
    action.finish(Err(ReceiveWorkError::Task(source.clone())));
    let error = stopped.wait().await.unwrap_err();
    let ReceiveWorkError::Task(retained) = error.as_ref() else {
        panic!("task source")
    };
    assert!(Arc::ptr_eq(retained, &source));
}
