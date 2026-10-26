use super::exchange::settle;
use crate::connectivity::{ConnectivityError, ElapsedClockError, ElapsedClockPort};
use crate::transfer::progress::{ProgressExchangeDeadline, ProgressReceiveError};
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
#[test]
fn received_budget_includes_preparation_and_clones_share_clock_retirement() {
    let clock = Arc::new(Clock(AtomicU64::new(3)));
    let deadline =
        ProgressExchangeDeadline::received(clock.clone(), Duration::ZERO, Duration::from_secs(5))
            .unwrap();
    let clone = deadline.clone();
    let cancel = CancellationToken::new();
    clock.0.store(5, Ordering::SeqCst);
    assert!(deadline.check(&cancel).is_err());
    clock.0.store(3, Ordering::SeqCst);
    assert!(clone.check(&cancel).is_err());
    assert!(ProgressExchangeDeadline::received(
        clock.clone(),
        Duration::from_secs(4),
        Duration::from_secs(5)
    )
    .is_err());
    clock.0.store(5, Ordering::SeqCst);
    assert!(
        ProgressExchangeDeadline::received(clock, Duration::ZERO, Duration::from_secs(5)).is_err()
    );
}
#[tokio::test]
async fn expired_exchange_never_polls_new_local_work() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let deadline = ProgressExchangeDeadline::new(clock.clone(), Duration::from_secs(5)).unwrap();
    clock.0.store(5, Ordering::SeqCst);
    let calls = AtomicUsize::new(0);
    let stopped = AtomicUsize::new(0);
    let result = settle(
        async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        &deadline,
        &CancellationToken::new(),
        || {
            stopped.fetch_add(1, Ordering::SeqCst);
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(ProgressReceiveError::Connection(
            ConnectivityError::DeadlineExpired
        ))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn expiry_waits_for_actual_work_and_preserves_late_failure_source() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let deadline = ProgressExchangeDeadline::new(clock.clone(), Duration::from_secs(5)).unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let started = entered.clone();
    let gate = release.clone();
    let task = tokio::spawn(async move {
        settle(
            async {
                started.add_permits(1);
                gate.acquire().await.unwrap().forget();
                Err::<(), _>(ProgressReceiveError::Exchange(Box::new(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "controlled local failure",
                ))))
            },
            &deadline,
            &CancellationToken::new(),
            || {},
        )
        .await
    });
    entered.acquire().await.unwrap().forget();
    clock.0.store(5, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!task.is_finished());
    release.add_permits(1);
    let error = task.await.unwrap().unwrap_err();
    assert!(error.source().is_some());
    let ProgressReceiveError::Settlement {
        primary,
        additional,
    } = error
    else {
        panic!("both failures must remain available")
    };
    assert!(matches!(
        *primary,
        ProgressReceiveError::Connection(ConnectivityError::DeadlineExpired)
    ));
    assert_eq!(
        additional
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[tokio::test]
async fn cancellation_does_not_report_the_same_stop_as_a_second_failure() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let deadline = ProgressExchangeDeadline::new(clock, Duration::from_secs(5)).unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let started = entered.clone();
    let gate = release.clone();
    let task = tokio::spawn(async move {
        settle(
            async {
                started.add_permits(1);
                gate.acquire().await.unwrap().forget();
                Err::<(), _>(ConnectivityError::Stopped.into())
            },
            &deadline,
            &stop,
            || {},
        )
        .await
    });
    entered.acquire().await.unwrap().forget();
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!task.is_finished());
    release.add_permits(1);
    assert!(matches!(
        task.await.unwrap(),
        Err(ProgressReceiveError::Connection(ConnectivityError::Stopped))
    ));
}
