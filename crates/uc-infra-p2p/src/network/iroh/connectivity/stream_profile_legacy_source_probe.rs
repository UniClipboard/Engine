//! 旧兼容沿原完整动作等待实际本地读取；取消、原期限或退休后不补发旧帧。
use super::{descriptor, terminal, Facts, LegacyPair, BUDGET};
use crate::network::iroh::transfer_progress_v2::TransferProgressWire;
use async_trait::async_trait;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectivityError, ElapsedClockError, ElapsedClockPort, ProgressClaimRefusal,
    ProgressExchangeDeadline, ProgressPageReply, ProgressReceiveError,
    ProgressSnapshotProviderPort, ProgressSnapshotQuery, ProgressSourceTask,
    ProgressTaskAuthorityPort, ProgressTerminalRoundPort, TransferProgressProducer,
};
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemState};
use uc_core::ids::DeviceId;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
struct HeldSnapshot {
    facts: Arc<Facts>,
    entered: Semaphore,
    released: Semaphore,
    failed: AtomicBool,
    retired: AtomicBool,
}
impl ProgressTaskAuthorityPort for HeldSnapshot {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        if self.retired.load(Ordering::SeqCst) {
            None
        } else {
            self.facts.current_task(peer, source)
        }
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressIdentity,
        _: ProgressIdentity,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("the complete legacy round cannot claim another actual result");
    }
}
#[async_trait]
impl ProgressSnapshotProviderPort for HeldSnapshot {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        self.entered.add_permits(1);
        self.released.acquire().await.unwrap().forget();
        if self.failed.load(Ordering::SeqCst) {
            return Err(ProgressReceiveError::Exchange(Box::new(io::Error::other(
                "test actual summary read failed",
            ))));
        }
        self.facts.snapshot_page(peer, query).await
    }
}
#[derive(Clone, Copy)]
enum End {
    Deadline,
    Cancel,
    Session,
    Retired,
    Failure,
}
async fn actual_read_ends_before_any_legacy_network(end: End) -> anyhow::Result<()> {
    timeout(BUDGET * 3, async {
        let mut pair = LegacyPair::new().await?;
        let peer = pair.peer;
        let facts = Arc::new(Facts {
            peer,
            items: vec![
                ProgressItemState::Terminal(terminal(0, false)),
                ProgressItemState::Terminal(terminal(1, false)),
            ],
            reads: AtomicUsize::new(0),
        });
        let held = Arc::new(HeldSnapshot {
            facts: facts.clone(),
            entered: Semaphore::new(0),
            released: Semaphore::new(0),
            failed: AtomicBool::new(false),
            retired: AtomicBool::new(false),
        });
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let producer = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            held.clone(),
            held.clone(),
            clock.clone(),
        );
        let stop = CancellationToken::new();
        let local_stop = stop.clone();
        let task_clock = clock.clone();
        let sending = tokio::spawn(async move {
            let deadline = ProgressExchangeDeadline::new(task_clock, BUDGET)?;
            producer
                .report_settled(
                    &peer,
                    descriptor(),
                    terminal(1, false),
                    &deadline,
                    &local_stop,
                )
                .await
        });
        timeout(BUDGET, held.entered.acquire()).await??.forget();
        let (remote, _) = timeout(BUDGET, pair.incoming.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual legacy connection required"))?;
        match end {
            End::Deadline => {
                clock.0.store(5, Ordering::SeqCst);
            }
            End::Cancel => stop.cancel(),
            End::Session => pair.outbound.disconnect_all().await?,
            End::Retired => {
                held.retired.store(true, Ordering::SeqCst);
            }
            End::Failure => {
                stop.cancel();
                held.failed.store(true, Ordering::SeqCst);
            }
        }
        tokio::task::yield_now().await;
        assert!(
            !sending.is_finished(),
            "actual local facts must remain supervised before returning"
        );
        held.released.add_permits(1);
        let result = sending.await?;
        let probe = CancellationToken::new();
        let accepted = timeout(
            Duration::from_millis(100),
            remote.accept_uni(Instant::now() + Duration::from_millis(100), &probe),
        )
        .await;
        let opened = matches!(accepted, Ok(Ok(_)));
        drop(accepted);
        drop(remote);
        pair.close().await?;
        assert!(
            !opened,
            "an expired, cancelled or retired summary started a new native stream"
        );
        assert_eq!(
            facts.reads.load(Ordering::SeqCst),
            if matches!(end, End::Failure) { 0 } else { 1 }
        );
        match end {
            End::Deadline => assert!(matches!(
                result,
                Err(ProgressReceiveError::Connection(
                    ConnectivityError::DeadlineExpired
                ))
            )),
            End::Cancel | End::Session => assert!(matches!(
                result,
                Err(ProgressReceiveError::Connection(ConnectivityError::Stopped))
            )),
            End::Retired => assert!(matches!(result, Err(ProgressReceiveError::Stale))),
            End::Failure => {
                let error = result.unwrap_err();
                assert!(matches!(&error, ProgressReceiveError::Exchange(_)));
                assert_eq!(
                    error
                        .source()
                        .unwrap()
                        .downcast_ref::<io::Error>()
                        .unwrap()
                        .kind(),
                    io::ErrorKind::Other
                );
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
#[tokio::test]
async fn legacy_summary_keeps_original_deadline_while_local_snapshot_is_reading(
) -> anyhow::Result<()> {
    actual_read_ends_before_any_legacy_network(End::Deadline).await
}
#[tokio::test]
async fn legacy_summary_cancel_waits_for_actual_read_but_never_opens_a_uni_stream(
) -> anyhow::Result<()> {
    actual_read_ends_before_any_legacy_network(End::Cancel).await
}
#[tokio::test]
async fn legacy_summary_old_session_cannot_send_after_actual_snapshot_read() -> anyhow::Result<()> {
    actual_read_ends_before_any_legacy_network(End::Session).await
}
#[tokio::test]
async fn legacy_summary_retired_attempt_cannot_send_its_completed_old_snapshot(
) -> anyhow::Result<()> {
    actual_read_ends_before_any_legacy_network(End::Retired).await
}
#[tokio::test]
async fn legacy_summary_actual_read_failure_keeps_source_even_after_cancellation(
) -> anyhow::Result<()> {
    actual_read_ends_before_any_legacy_network(End::Failure).await
}
