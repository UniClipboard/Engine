use std::error::Error as _;
use std::io::{Error as IoError, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Notify};
use tokio::task::{spawn_blocking, JoinError};
use tokio::time::timeout;

use crate::facade::clipboard_history::HistoryMaintenanceRuntimeError;
use crate::search::SearchShutdownError;

use super::{begin, finish, ApplicationShutdown, LifecycleError};

#[tokio::test]
async fn stopping_domains_preserves_each_typed_failure() {
    let history = tokio::spawn(async { panic!("PRIVATE_HISTORY_FAILURE") })
        .await
        .unwrap_err();
    let timeout = tokio::spawn(std::future::pending::<()>());
    timeout.abort();
    let clipboard = Arc::new(LifecycleError {
        primary: IoError::other("private clipboard").into(),
        additional: Vec::new(),
    });
    let space = Arc::new(LifecycleError {
        primary: IoError::other("private space").into(),
        additional: Vec::new(),
    });
    let tasks = vec![
        begin("stop history", async move {
            Err(HistoryMaintenanceRuntimeError::Task(history))
        }),
        begin(
            "stop timeout",
            async move { Err(timeout.await.unwrap_err()) },
        ),
        begin("stop search", async {
            Err(SearchShutdownError::Coordinator {
                source: IoError::other("PRIVATE_SEARCH_FAILURE").into(),
            })
        }),
        begin("stop clipboard", {
            let source = Arc::clone(&clipboard);
            async move { Err(source) }
        }),
        begin("stop active clipboard", async {
            Err(LifecycleError {
                primary: IoError::other("private active clipboard").into(),
                additional: Vec::new(),
            })
        }),
        begin("stop space", {
            let source = Arc::clone(&space);
            async move { Err(source) }
        }),
    ];
    let failure = finish(tasks).await.unwrap_err();
    assert!(failure.source().is_some());
    assert!(failure
        .primary
        .downcast_ref::<HistoryMaintenanceRuntimeError>()
        .is_some());
    assert_eq!(failure.additional.len(), 5);
    assert!(failure.additional[0]
        .downcast_ref::<JoinError>()
        .unwrap()
        .is_cancelled());
    assert!(failure.additional[1]
        .downcast_ref::<SearchShutdownError>()
        .unwrap()
        .source()
        .is_some());
    assert!(Arc::ptr_eq(
        &clipboard,
        failure.additional[2]
            .downcast_ref::<Arc<LifecycleError>>()
            .unwrap()
    ));
    assert!(failure.additional[3]
        .downcast_ref::<LifecycleError>()
        .is_some());
    assert!(Arc::ptr_eq(
        &space,
        failure.additional[4]
            .downcast_ref::<Arc<LifecycleError>>()
            .unwrap()
    ));
    assert!(!format!("{failure:?}").contains("PRIVATE"));
}

async fn panicking_cleanup() -> Result<(), IoError> {
    panic!("private domain shutdown panic");
}

#[tokio::test]
async fn one_slow_domain_does_not_delay_other_shutdowns_or_hide_panics() {
    let entered = Arc::new(Notify::new());
    let quick = Arc::new(Notify::new());
    let (release, blocked) = oneshot::channel();
    let tasks = vec![
        begin("stop slow domain", {
            let entered = Arc::clone(&entered);
            async move {
                spawn_blocking(move || {
                    entered.notify_one();
                    blocked.blocking_recv().unwrap();
                })
                .await
                .unwrap();
                Ok::<(), IoError>(())
            }
        }),
        begin("stop quick domain", {
            let quick = Arc::clone(&quick);
            async move {
                quick.notify_one();
                Err(IoError::other("private quick failure"))
            }
        }),
        begin("stop panicking domain", panicking_cleanup()),
    ];
    let mut closing = tokio::spawn(finish(tasks));
    entered.notified().await;
    let notified = timeout(Duration::from_secs(1), quick.notified()).await;
    let early = timeout(Duration::from_millis(20), &mut closing).await;
    release.send(()).unwrap();
    assert!(notified.is_ok());
    assert!(early.is_err());
    let failure = closing.await.unwrap().unwrap_err();
    assert!(failure.primary.downcast_ref::<IoError>().is_some());
    assert_eq!(failure.additional.len(), 1);
    assert!(failure.additional[0]
        .downcast_ref::<JoinError>()
        .unwrap()
        .is_panic());
}

#[tokio::test]
async fn cancelled_waiter_and_repeated_shutdown_share_the_complete_cleanup() {
    let shutdown = Arc::new(ApplicationShutdown::default());
    let entered = Arc::new(Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let (release, blocked) = oneshot::channel();
    let first = tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        let entered = Arc::clone(&entered);
        let finished = Arc::clone(&finished);
        async move {
            shutdown
                .run(async move {
                    spawn_blocking(move || {
                        entered.notify_one();
                        blocked.blocking_recv().unwrap();
                        finished.store(true, Ordering::SeqCst);
                    })
                    .await
                    .unwrap();
                    Ok(())
                })
                .await
        }
    });
    entered.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let mut repeated = Box::pin(shutdown.run(async { panic!("cleanup must only run once") }));
    let early = timeout(Duration::from_millis(20), repeated.as_mut()).await;
    let premature = finished.load(Ordering::SeqCst);
    release.send(()).unwrap();
    assert!(early.is_err());
    assert!(!premature);
    repeated.await.unwrap();
    assert!(finished.load(Ordering::SeqCst));
    shutdown
        .run(async { panic!("completed cleanup must not rerun") })
        .await
        .unwrap();
}

#[tokio::test]
async fn repeated_shutdown_keeps_the_original_failure() {
    let shutdown = ApplicationShutdown::default();
    let first = shutdown
        .run(async {
            Err(LifecycleError {
                primary: IoError::new(ErrorKind::PermissionDenied, "private cleanup source").into(),
                additional: Vec::new(),
            })
        })
        .await
        .unwrap_err();
    let second = shutdown.run(async { Ok(()) }).await.unwrap_err();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(
        second.primary.downcast_ref::<IoError>().unwrap().kind(),
        ErrorKind::PermissionDenied
    );
    assert!(!format!("{second:?}").contains("private"));
}

#[tokio::test]
async fn a_panicking_cleanup_never_becomes_a_successful_repeat() {
    let shutdown = ApplicationShutdown::default();
    let first = shutdown
        .run(async { panic!("private cleanup panic") })
        .await
        .unwrap_err();
    let second = shutdown.run(async { Ok(()) }).await.unwrap_err();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(second
        .primary
        .downcast_ref::<JoinError>()
        .unwrap()
        .is_panic());
    assert!(!format!("{second:?}").contains("private"));
}
