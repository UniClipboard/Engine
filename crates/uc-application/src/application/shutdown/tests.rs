use std::io::{Error as IoError, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Notify};
use tokio::task::{spawn_blocking, JoinError};
use tokio::time::timeout;

use super::{ApplicationShutdown, LifecycleError};

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
