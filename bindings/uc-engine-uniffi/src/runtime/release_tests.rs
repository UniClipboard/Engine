//! Controlled binding-owner scenarios; these do not certify real blob descriptors.
use super::{EventQueue, LifecycleCommand, MobileEngine, WorkerJoin};
use crate::{BindingError, BindingErrorCategory, BindingResourceRelease};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

fn evidence(name: &str, value: &str) {
    if let Some(root) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(std::path::PathBuf::from(root).join(name), value).unwrap();
    }
}

#[test]
fn terminal_worker_failure_is_not_pending_on_repeated_waits() {
    let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
    let (lifecycle, lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
    drop(requests);
    drop(lifecycle_requests);
    let events = Arc::new(EventQueue::new(1));
    events.close();
    let error = BindingError::Engine {
        code: 1108,
        category: BindingErrorCategory::Internal,
        retryable: true,
    };
    let failure = error.clone();
    let worker = thread::spawn(move || Err::<(), _>(failure));
    let engine = MobileEngine {
        commands: Mutex::new(Some(commands)),
        lifecycle_commands: Mutex::new(Some(lifecycle)),
        shutdown_pending: AtomicBool::new(false),
        shutdown_wait: Mutex::new(None),
        events,
        worker: WorkerJoin::new(worker),
    };
    let expected = BindingResourceRelease::Failed { error };
    assert_eq!(engine.shutdown_with_outcome(1000), expected);
    assert_eq!(engine.shutdown_with_outcome(1000), expected);
    assert_eq!(engine.suspend_with_outcome(1000), expected);
    evidence(
        "controlled-terminal.txt",
        "failed-unconfirmed; repeated wait remains terminal; no release inferred",
    );
}

#[test]
fn caller_timeout_then_suspend_waits_for_the_same_shutdown() {
    let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
    let (lifecycle, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
    let events = Arc::new(EventQueue::new(1));
    let queue = events.clone();
    let (release, resume) = mpsc::channel();
    let worker = thread::spawn(move || {
        drop(requests);
        let Some(LifecycleCommand::Shutdown { response, .. }) = lifecycle_requests.blocking_recv()
        else {
            panic!("shutdown missing")
        };
        resume.recv().unwrap();
        queue.close();
        let _ = response.send(Ok(()).into());
        Ok(())
    });
    let engine = MobileEngine {
        commands: Mutex::new(Some(commands)),
        lifecycle_commands: Mutex::new(Some(lifecycle)),
        shutdown_pending: AtomicBool::new(false),
        shutdown_wait: Mutex::new(None),
        events,
        worker: WorkerJoin::new(worker),
    };
    assert_eq!(
        engine.shutdown_with_outcome(1),
        BindingResourceRelease::Pending
    );
    release.send(()).unwrap();
    let released = BindingResourceRelease::Released { error: None };
    assert_eq!(engine.suspend_with_outcome(1000), released);
    assert_eq!(engine.shutdown_with_outcome(1000), released);
    evidence(
        "controlled-late.txt",
        "pending; same worker late completion joined; repeated shutdown released",
    );
    thread::sleep(Duration::from_millis(1));
}
