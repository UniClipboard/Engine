//! 真实旧profile只向原Application投影提交未关联通知；帧与FIN前不开始采用。
use super::super::ProfilePair;
use super::{LegacyPair, BUDGET, SOURCE};
use crate::network::iroh::transfer_progress_v2::TransferProgressInbound;
use async_trait::async_trait;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ConnectionProtocol, ConnectivityError, ElapsedClockError, ElapsedClockPort,
    PeerReceiveStream, PeerSendStream, ProgressExchangeDeadline, ProgressProjectionKey,
    ProgressProjectionRegistry, ProgressProjectionWorker, ProgressReceiveError,
    ProgressUnattributedReceiverPort, UniStreamLease,
};
use uc_core::file_transfer::{
    FileTransferCancellationReason, OutboundProgressStatus, ProgressBatchState,
    ProgressFreshnessView, ProgressIdentity, ProgressItem, ProgressItemKind,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_sync_protocol::transfer_progress::{
    write_frame, ProgressFrame, ProgressWireError, FRAME_LEN,
};
use uuid::Uuid;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
struct Owner {
    registry: ProgressProjectionRegistry,
    calls: AtomicUsize,
    paused: AtomicBool,
    failed: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait]
impl ProgressUnattributedReceiverPort for Owner {
    async fn observe_unattributed(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Result<(), ProgressReceiveError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.paused.load(Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release
                .acquire()
                .await
                .map_err(|source| ProgressReceiveError::Exchange(Box::new(source)))?
                .forget();
            if self.failed.load(Ordering::SeqCst) {
                return Err(ProgressReceiveError::Exchange(Box::new(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "controlled adoption failure",
                ))));
            }
        }
        self.registry.observe_unattributed(peer, source).await
    }
}
struct Round {
    pair: LegacyPair,
    local: ConnectionLease,
    remote: ConnectionLease,
    send: Option<UniStreamLease<Box<dyn PeerSendStream>>>,
    receive: Option<UniStreamLease<Box<dyn PeerReceiveStream>>>,
    owner: Arc<Owner>,
    key: ProgressProjectionKey,
    clock: Arc<Clock>,
    _worker: ProgressProjectionWorker,
}
impl Round {
    async fn new(payload: &[u8], finish: bool) -> anyhow::Result<Self> {
        let mut pair = LegacyPair::new().await?;
        let stop = CancellationToken::new();
        let local = pair
            .outbound
            .request(pair.peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let (remote, _) = timeout(BUDGET, pair.incoming.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual legacy admission required"))?;
        assert_eq!(local.protocol(), ConnectionProtocol::TransferProgressV1);
        assert_eq!(remote.protocol(), ConnectionProtocol::TransferProgressV1);
        let mut send = local.open_uni(Instant::now() + BUDGET, &stop).await?;
        send.stream.write_all(payload).await?;
        if finish {
            send.stream.finish()?;
        }
        let receive = remote.accept_uni(Instant::now() + BUDGET, &stop).await?;
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let (registry, worker) = ProgressProjectionRegistry::new(clock.clone());
        let key = ProgressProjectionKey {
            peer: remote.peer(),
            source: SOURCE,
        };
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string(),
            ))
            .accept(
                key,
                (0..2)
                    .map(|index| ProgressItem {
                        kind: ProgressItemKind::File,
                        index,
                    })
                    .collect(),
            )?;
        let owner = Arc::new(Owner {
            registry,
            calls: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        Ok(Self {
            pair,
            local,
            remote,
            send: Some(send),
            receive: Some(receive),
            owner,
            key,
            clock,
            _worker: worker,
        })
    }
    fn deadline(&self) -> ProgressExchangeDeadline {
        ProgressExchangeDeadline::new(self.clock.clone(), BUDGET).unwrap()
    }
    async fn close(mut self) -> anyhow::Result<()> {
        drop(self.send.take());
        drop(self.receive.take());
        drop(self.local);
        drop(self.remote);
        self.pair.close().await
    }
}
async fn payload(status: OutboundProgressStatus) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_frame(
        &mut bytes,
        &ProgressFrame {
            transfer_id_bytes: SOURCE.bytes(),
            bytes_transferred: 17,
            total_bytes: Some(17),
            status,
        },
    )
    .await
    .unwrap();
    assert_eq!(bytes.len(), FRAME_LEN);
    bytes
}

#[tokio::test]
async fn legacy_inbound_all_old_statuses_only_mark_the_actual_original_intent_unconfirmed(
) -> anyhow::Result<()> {
    for status in [
        OutboundProgressStatus::InProgress,
        OutboundProgressStatus::Completed,
        OutboundProgressStatus::Failed,
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::LocalUser,
        },
    ] {
        let mut round = Round::new(&payload(status).await, true).await?;
        let stop = CancellationToken::new();
        let result = TransferProgressInbound::serve_legacy(
            round.receive.take().unwrap(),
            &round.remote,
            round.deadline(),
            &stop,
            round.owner.as_ref(),
        )
        .await;
        let view = round.owner.registry.snapshot(round.key).unwrap();
        let calls = round.owner.calls.load(Ordering::SeqCst);
        round.close().await?;
        result?;
        assert_eq!(calls, 1);
        assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Unconfirmed);
        assert_eq!(view.summary.state, ProgressBatchState::Unconfirmed);
        assert_eq!(view.summary.counts.unconfirmed, 2);
        assert!(view.task.is_none() && view.bound.is_none());
        assert!(view.items.iter().all(|(_, value)| value.is_none()));
    }
    Ok(())
}

#[tokio::test]
async fn legacy_inbound_rejects_bad_magic_status_truncation_and_trailing_bytes_before_adoption(
) -> anyhow::Result<()> {
    let original = payload(OutboundProgressStatus::Completed).await;
    for case in 0..4 {
        let mut bytes = original.clone();
        match case {
            0 => bytes[0] = 0,
            1 => bytes[FRAME_LEN - 1] = 0,
            2 => {
                bytes.pop();
            }
            _ => bytes.push(0),
        }
        let mut round = Round::new(&bytes, true).await?;
        let stop = CancellationToken::new();
        let result = TransferProgressInbound::serve_legacy(
            round.receive.take().unwrap(),
            &round.remote,
            round.deadline(),
            &stop,
            round.owner.as_ref(),
        )
        .await;
        let calls = round.owner.calls.load(Ordering::SeqCst);
        let view = round.owner.registry.snapshot(round.key).unwrap();
        round.close().await?;
        assert_eq!(calls, 0);
        assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Waiting);
        if case == 2 {
            let error = result.unwrap_err();
            assert!(matches!(error, ProgressReceiveError::Exchange(_)));
            assert!(matches!(
                error.source().unwrap().downcast_ref::<ProgressWireError>(),
                Some(ProgressWireError::Io(_))
            ));
        } else {
            assert!(matches!(result, Err(ProgressReceiveError::Malformed(_))));
        }
    }
    Ok(())
}

#[tokio::test]
async fn legacy_inbound_authorized_peer_cannot_update_another_peers_intent() -> anyhow::Result<()> {
    let mut round = Round::new(&payload(OutboundProgressStatus::Completed).await, true).await?;
    round.owner.registry.remove(round.key);
    let other = ProgressProjectionKey {
        peer: DeviceId::new("unrelated-peer"),
        source: SOURCE,
    };
    round
        .owner
        .registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((other).source.bytes()).to_string(),
        ))
        .accept(
            other,
            vec![ProgressItem {
                kind: ProgressItemKind::File,
                index: 0,
            }],
        )?;
    let stop = CancellationToken::new();
    let result = TransferProgressInbound::serve_legacy(
        round.receive.take().unwrap(),
        &round.remote,
        round.deadline(),
        &stop,
        round.owner.as_ref(),
    )
    .await;
    assert!(round.owner.registry.snapshot(round.key).is_none());
    let view = round.owner.registry.snapshot(other).unwrap();
    let count = round.owner.registry.snapshots().len();
    round.close().await?;
    assert!(matches!(result, Err(ProgressReceiveError::UnknownTask)));
    assert_eq!(count, 1);
    assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Waiting);
    Ok(())
}

#[tokio::test]
async fn legacy_inbound_cannot_downgrade_an_actual_new_protocol_connection() -> anyhow::Result<()> {
    let pair = ProfilePair::new().await?;
    let stop = CancellationToken::new();
    let local = pair
        .outbound
        .request(pair.peer)
        .connect(Instant::now() + BUDGET, &stop)
        .await?;
    let mut send = local.open_uni(Instant::now() + BUDGET, &stop).await?;
    send.stream
        .write_all(&payload(OutboundProgressStatus::Completed).await)
        .await?;
    send.stream.finish()?;
    let receive = pair
        .receiving
        .accept_uni(Instant::now() + BUDGET, &stop)
        .await?;
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (registry, _worker) = ProgressProjectionRegistry::new(clock.clone());
    let owner = Owner {
        registry,
        calls: AtomicUsize::new(0),
        paused: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    };
    assert_eq!(
        pair.receiving.protocol(),
        ConnectionProtocol::TransferProgressV2
    );
    let result = TransferProgressInbound::serve_legacy(
        receive,
        &pair.receiving,
        ProgressExchangeDeadline::new(clock, BUDGET)?,
        &stop,
        &owner,
    )
    .await;
    drop(send);
    drop(local);
    pair.close().await?;
    assert!(matches!(result, Err(ProgressReceiveError::Protocol)));
    assert_eq!(owner.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

enum End {
    Finish,
    Cancel,
    Deadline,
    Session,
    Remove,
}
async fn pending_fin(end: End) -> anyhow::Result<()> {
    let mut round = Round::new(&payload(OutboundProgressStatus::Completed).await, false).await?;
    let stop = CancellationToken::new();
    let result = {
        let serving = TransferProgressInbound::serve_legacy(
            round.receive.take().unwrap(),
            &round.remote,
            round.deadline(),
            &stop,
            round.owner.as_ref(),
        );
        tokio::pin!(serving);
        assert!(timeout(Duration::from_millis(50), &mut serving)
            .await
            .is_err());
        assert_eq!(round.owner.calls.load(Ordering::SeqCst), 0);
        match end {
            End::Cancel => stop.cancel(),
            End::Deadline => round.clock.0.store(5, Ordering::SeqCst),
            End::Session => round.pair.inbound.disconnect_all().await?,
            End::Finish => round.send.as_mut().unwrap().stream.finish()?,
            End::Remove => {
                round.owner.registry.remove(round.key);
                round.send.as_mut().unwrap().stream.finish()?;
            }
        }
        timeout(BUDGET, &mut serving).await?
    };
    let calls = round.owner.calls.load(Ordering::SeqCst);
    let view = round.owner.registry.snapshot(round.key);
    round.close().await?;
    match end {
        End::Finish => {
            result?;
            assert_eq!(calls, 1);
            assert_eq!(
                view.unwrap().freshness.unwrap(),
                ProgressFreshnessView::Unconfirmed
            );
        }
        End::Remove => {
            assert!(matches!(result, Err(ProgressReceiveError::UnknownTask)));
            assert_eq!(calls, 1);
            assert!(view.is_none());
        }
        End::Cancel => {
            assert!(matches!(
                result,
                Err(ProgressReceiveError::Connection(ConnectivityError::Stopped))
            ));
            assert_eq!(calls, 0);
        }
        End::Deadline => {
            assert!(matches!(
                result,
                Err(ProgressReceiveError::Connection(
                    ConnectivityError::DeadlineExpired
                ))
            ));
            assert_eq!(calls, 0);
        }
        End::Session => {
            assert!(result.is_err());
            assert_eq!(calls, 0);
        }
    }
    Ok(())
}
#[tokio::test]
async fn legacy_inbound_waits_for_actual_fin_before_using_the_original_intent() -> anyhow::Result<()>
{
    pending_fin(End::Finish).await
}
#[tokio::test]
async fn legacy_inbound_cancel_during_fin_never_starts_adoption() -> anyhow::Result<()> {
    pending_fin(End::Cancel).await
}
#[tokio::test]
async fn legacy_inbound_original_elapsed_deadline_still_governs_pending_fin() -> anyhow::Result<()>
{
    pending_fin(End::Deadline).await
}
#[tokio::test]
async fn legacy_inbound_retired_connection_cannot_cross_pending_fin() -> anyhow::Result<()> {
    pending_fin(End::Session).await
}
#[tokio::test]
async fn legacy_inbound_deleted_intent_cannot_be_recreated_after_pending_fin() -> anyhow::Result<()>
{
    pending_fin(End::Remove).await
}

#[tokio::test]
async fn legacy_inbound_keeps_started_adoption_alive_after_cancel_and_preserves_its_failure(
) -> anyhow::Result<()> {
    for failed in [false, true] {
        let mut round = Round::new(&payload(OutboundProgressStatus::Completed).await, true).await?;
        round.owner.paused.store(true, Ordering::SeqCst);
        round.owner.failed.store(failed, Ordering::SeqCst);
        let stop = CancellationToken::new();
        let result = {
            let serving = TransferProgressInbound::serve_legacy(
                round.receive.take().unwrap(),
                &round.remote,
                round.deadline(),
                &stop,
                round.owner.as_ref(),
            );
            tokio::pin!(serving);
            tokio::select! {
                permit = round.owner.entered.acquire() => permit?.forget(),
                result = &mut serving => panic!("adoption returned before release: {result:?}"),
            }
            stop.cancel();
            round.clock.0.store(5, Ordering::SeqCst);
            assert!(timeout(Duration::from_millis(50), &mut serving)
                .await
                .is_err());
            assert!(!round.remote.is_closed());
            round.owner.release.add_permits(1);
            timeout(BUDGET, &mut serving).await?
        };
        let view = round.owner.registry.snapshot(round.key).unwrap();
        round.close().await?;
        if failed {
            let error = result.unwrap_err();
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Waiting);
        } else {
            result?;
            assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Unconfirmed);
        }
    }
    Ok(())
}
