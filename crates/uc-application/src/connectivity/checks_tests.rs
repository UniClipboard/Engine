use super::checked;
use crate::connectivity::ConnectivityError;
use futures::poll;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio::time::{advance, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;

#[tokio::test(start_paused = true)]
async fn cancelled_or_expired_unstarted_check_is_never_started_for_settlement() {
    for expired in [false, true] {
        let started = AtomicBool::new(false);
        let work = async {
            started.store(true, Ordering::SeqCst);
            Ok(())
        };
        let cancel = CancellationToken::new();
        if !expired {
            cancel.cancel();
        }
        let deadline = Instant::now()
            + if expired {
                Duration::ZERO
            } else {
                Duration::from_secs(10)
            };
        let Err(error) = checked(work, true, deadline, &cancel).await else {
            panic!("late check accepted");
        };
        assert_eq!(
            error.class(),
            if expired {
                "connection_admission_timeout"
            } else {
                "connection_stopped"
            }
        );
        assert!(!started.load(Ordering::SeqCst));
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_timeout_retain_the_actual_read_failure_and_wait_for_it() {
    for expire in [false, true] {
        let entered = Arc::new(Notify::new());
        let finish = Arc::new(Semaphore::new(0));
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let signal = entered.clone();
        let gate = finish.clone();
        let mut checking = tokio::spawn(async move {
            checked(
                async move {
                    signal.notify_one();
                    let _work = gate.acquire().await.expect("read gate");
                    Err::<(), _>(ConnectivityError::Admission(Box::new(io::Error::from(
                        io::ErrorKind::PermissionDenied,
                    ))))
                },
                true,
                Instant::now() + Duration::from_secs(1),
                &token,
            )
            .await
        });
        entered.notified().await;
        if expire {
            advance(Duration::from_secs(1)).await;
        } else {
            cancel.cancel();
        }
        tokio::task::yield_now().await;
        assert!(
            poll!(&mut checking).is_pending(),
            "logical stop released actual read"
        );
        finish.add_permits(1);
        let Err(error) = checking.await.expect("check task") else {
            panic!("failed read accepted");
        };
        assert_eq!(error.class(), "connection_settlement_failed");
        assert!(error.is_admission_read_failure());
        let ConnectivityError::Settlement {
            primary,
            additional,
        } = error
        else {
            panic!("compound source lost");
        };
        assert!(if expire {
            matches!(*primary, ConnectivityError::AdmissionTimedOut(_))
        } else {
            matches!(*primary, ConnectivityError::Stopped)
        });
        let source = additional.source().expect("read source");
        assert_eq!(
            source
                .downcast_ref::<io::Error>()
                .expect("original IO source")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
