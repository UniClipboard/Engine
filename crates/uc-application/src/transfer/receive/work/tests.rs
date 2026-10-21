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
    a.register(&entry, "attempt-a").unwrap();
    b.register(&entry, "attempt-b").unwrap();
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
    action.register(&entry, "attempt").unwrap();
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
    action.register(&entry, "attempt").unwrap();
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

#[tokio::test]
async fn closing_generation_seals_old_actions_without_forgetting_actual_settlement() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("entry");
    let unregistered = registry.action();
    let receiving = registry.action();
    assert!(receiving.register(&entry, "old").is_ok());
    let child = receiving.cancellation();
    registry.close();
    assert!(child.is_cancelled());
    assert!(!registry.is_open());
    assert!(registry.owns_entry(entry.as_ref()));
    let mut stopped = Box::pin(registry.stop(&entry, "old").unwrap().wait());
    assert!(timeout(Duration::from_millis(10), stopped.as_mut())
        .await
        .is_err());

    let current = registry.generation();
    assert!(registry.open(&current));
    assert!(unregistered.register(&entry, "late").is_err());
    assert!(unregistered.cancellation().is_cancelled());
    let next = registry.action();
    assert!(next.register(&EntryId::from("next-entry"), "next").is_ok());
    assert!(!next.cancellation().is_cancelled());
    receiving.finish(Ok(()));
    stopped.await.unwrap();
    assert!(!registry.owns_entry(entry.as_ref()));
    assert!(registry.owns_entry("next-entry"));
    next.finish(Ok(()));
}

#[test]
fn a_failed_action_releases_sweep_ownership_only_after_actual_finish() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("entry");
    let action = registry.action();
    assert!(action.register(&entry, "attempt").is_ok());
    assert!(registry.owns_entry(entry.as_ref()));
    action.cancellation().cancel();
    assert!(registry.owns_entry(entry.as_ref()));
    action.finish(Err(ReceiveWorkError::Action(
        io::Error::from(io::ErrorKind::StorageFull).into(),
    )));
    assert!(!registry.owns_entry(entry.as_ref()));
}

#[test]
fn cleanup_and_new_receive_registration_cannot_overtake_each_other() {
    let registry = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("entry");
    let cleanup = registry.claim_cleanup(entry.as_ref()).unwrap();
    assert!(registry.claim_cleanup(entry.as_ref()).is_none());
    let receive = registry.action();
    assert!(matches!(
        receive.register(&entry, "new"),
        Err(super::ReceiveRegistrationError::CleanupInProgress)
    ));
    let other = registry.action();
    other.register(&EntryId::from("other"), "other").unwrap();
    drop(cleanup);
    receive.register(&entry, "new").unwrap();
    assert!(registry.claim_cleanup(entry.as_ref()).is_none());
    receive.finish(Ok(()));
    assert!(registry.claim_cleanup(entry.as_ref()).is_some());
    other.finish(Ok(()));
}
