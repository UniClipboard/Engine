//! 同一真实codec和Application动作使用第二backend；不把内存字节流当作真实网络或密码握手证据。
#[path = "stream_memory_backend_fixture.rs"]
mod fixture;
#[path = "stream_memory_backend_queued_query_probe.rs"]
mod queued_query_probe;
#[path = "stream_memory_backend_request_end_probe.rs"]
mod request_end_probe;
#[path = "stream_memory_backend_retirement_probe.rs"]
mod retirement_probe;
#[path = "stream_memory_backend_session_probe.rs"]
mod session_probe;
use super::progress_probe::{progress_budget, UnusedSnapshotProvider};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use async_trait::async_trait;
use fixture::MemoryPair;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectivityError, ProgressClaimRefusal, ProgressExchangeDeadline, ProgressExchangeService,
    ProgressLiveRoundPort, ProgressPageReply, ProgressProjectionKey, ProgressProjectionRegistry,
    ProgressReceiveError, ProgressSnapshotPage, ProgressSnapshotProviderPort,
    ProgressSnapshotQuery, ProgressSnapshotRead, ProgressSourceTask, ProgressTaskAuthorityPort,
    ProgressTaskDescriptor, ProgressTerminalRoundPort, TransferProgressProducer,
    TransferProgressReconciler, TransferProgressSnapshotReader,
};
use uc_core::file_transfer::{
    ProgressActive, ProgressFreshnessView, ProgressIdentity, ProgressItem, ProgressItemKind,
    ProgressItemState, ProgressPhase, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;

const SOURCE: ProgressIdentity = ProgressIdentity::new([1; 16]);
fn item(index: u16) -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index,
    }
}
fn descriptor() -> ProgressTaskDescriptor {
    ProgressTaskDescriptor {
        producer: ProgressIdentity::new([2; 16]),
        revision: 1,
        source: SOURCE,
        attempt: ProgressIdentity::new([3; 16]),
        item_count: 65,
    }
}
fn terminal(index: u16) -> ProgressTerminal {
    ProgressTerminal {
        item: item(index),
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 16,
        total: Some(16),
        outcome: ProgressTerminalOutcome::Completed,
    }
}
struct Authority {
    peer: DeviceId,
}
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        (peer == &self.peer && source == SOURCE).then(|| ProgressSourceTask {
            descriptor: descriptor(),
            items: (0..65).map(item).collect::<Vec<_>>().into(),
            notification_closed: Arc::new(HashSet::new()),
        })
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("complete round fixture does not claim local results");
    }
}
struct Snapshot {
    peer: DeviceId,
    pages: AtomicUsize,
}
#[async_trait]
impl ProgressSnapshotProviderPort for Snapshot {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        assert_eq!(*peer, self.peer);
        assert_eq!(query.source, SOURCE);
        assert!(query.attempt == Some(descriptor().attempt));
        let page = self.pages.fetch_add(1, Ordering::SeqCst);
        assert_eq!(usize::from(query.offset), page * 64);
        assert_eq!(query.revision, if page == 0 { None } else { Some(9) });
        Ok(ProgressPageReply::Current(ProgressSnapshotPage {
            descriptor: descriptor(),
            revision: 9,
            offset: query.offset,
            total: 65,
            items: (query.offset..(query.offset + 64).min(65))
                .map(|index| ProgressItemState::Terminal(terminal(index)))
                .collect(),
        }))
    }
}
struct UnusedNotifications;
impl ProgressTaskAuthorityPort for UnusedNotifications {
    fn current_task(&self, _: &DeviceId, _: ProgressIdentity) -> Option<ProgressSourceTask> {
        panic!("query cannot read local notification authority");
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("query cannot claim notification results");
    }
}
#[async_trait]
impl ProgressTerminalRoundPort for UnusedNotifications {
    async fn report_settled(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("query cannot send a terminal notification");
    }
}
#[async_trait]
impl ProgressLiveRoundPort for UnusedNotifications {
    async fn report_contact(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: u64,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("query cannot send contact");
    }
    async fn report_live(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressActive,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("query cannot send intermediate updates");
    }
}
fn denied(error: &ConnectivityError) -> bool {
    match error {
        ConnectivityError::Denied => true,
        ConnectivityError::Shared(error) => denied(error),
        _ => false,
    }
}
#[tokio::test]
async fn memory_backend_runs_actual_registration_notifications_ack_and_complete_snapshot_without_iroh(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let pair = MemoryPair::new().await?;
        let clock = Arc::new(SystemElapsedClock);
        let (registry, projection_worker) = ProgressProjectionRegistry::new(clock.clone());
        let key = ProgressProjectionKey { peer: pair.receiver_id, source: SOURCE };
        registry.accept_intent(key, (0..65).map(item).collect())?;
        let stop = CancellationToken::new();
        let projections = tokio::spawn(projection_worker.run(stop.clone()));
        let producer = TransferProgressProducer::new(pair.receiver.clone(), Arc::new(TransferProgressWire), Arc::new(Authority { peer: pair.sender_id }), Arc::new(UnusedSnapshotProvider), clock.clone());
        let serve = async {
            for _ in 0..6 {
                TransferProgressInbound::serve(pair.at_sender.accept_bi().await?, &pair.at_sender, 1, progress_budget(), &stop, &registry, &UnusedSnapshotProvider).await?;
            }
            anyhow::Ok(())
        };
        let notify = async {
            producer.report_contact(&pair.sender_id, descriptor(), 1, &progress_budget(), &stop).await?;
            let view = registry.snapshot(key).unwrap();
            assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Live);
            assert!(view.items.iter().all(|(_, item)| item.is_none()));
            let active = ProgressActive { item: item(1), round: 0, sequence: 1, bytes: 8, total: Some(16), phase: ProgressPhase::Transferring };
            producer.report_live(&pair.sender_id, descriptor(), active, &progress_budget(), &stop).await?;
            assert_eq!(registry.snapshot(key).unwrap().items[1].1, Some(ProgressItemState::Active(active)));
            producer.report_settled(&pair.sender_id, descriptor(), terminal(0), &progress_budget(), &stop).await?;
            assert_eq!(registry.snapshot(key).unwrap().summary.counts.completed, 1);
            anyhow::Ok(())
        };
        tokio::try_join!(serve, notify)?;
        assert_eq!(pair.receiver_dials.load(Ordering::SeqCst), 1);
        let snapshot = Snapshot { peer: pair.sender_id, pages: AtomicUsize::new(0) };
        let unused = Arc::new(UnusedNotifications);
        let (service, worker) = ProgressExchangeService::new(clock, unused.clone(), unused.clone(), Arc::new(TransferProgressSnapshotReader::new(pair.sender.clone(), Arc::new(TransferProgressWire))), unused);
        let reconciler = TransferProgressReconciler::new(registry.clone(), service);
        let queries = tokio::spawn(worker.run(stop.clone()));
        let serve = async {
            for _ in 0..2 {
                TransferProgressInbound::serve(pair.at_receiver.accept_bi().await?, &pair.at_receiver, 1, progress_budget(), &stop, &registry, &snapshot).await?;
            }
            anyhow::Ok(())
        };
        let read = async {
            let result = reconciler.refresh(key, &stop).await;
            assert!(matches!(result.as_ref(), Ok(ProgressSnapshotRead::Complete(_))));
            anyhow::Ok(())
        };
        tokio::try_join!(serve, read)?;
        assert_eq!(snapshot.pages.load(Ordering::SeqCst), 2);
        assert_eq!(pair.sender_dials.load(Ordering::SeqCst), 1);
        let before = registry.snapshot(key).unwrap();
        assert_eq!(before.summary.counts.completed, 65);
        assert_eq!(before.freshness.as_ref().unwrap(), &ProgressFreshnessView::Settled);
        pair.allowed.store(false, Ordering::SeqCst);
        let rejected = reconciler.refresh(key, &stop).await;
        assert!(matches!(rejected.as_ref(), Err(ProgressReceiveError::Connection(error)) if denied(error)));
        assert_eq!(snapshot.pages.load(Ordering::SeqCst), 2);
        assert_eq!(pair.sender_dials.load(Ordering::SeqCst), 1);
        let after = registry.snapshot(key).unwrap();
        assert_eq!(after.version, before.version);
        assert_eq!(after.items, before.items);
        registry.close();
        stop.cancel();
        projections.await?;
        queries.await?;
        pair.close().await?;
        anyhow::Ok(())
    }).await??;
    Ok(())
}
#[tokio::test]
async fn memory_backend_stopping_one_exchange_keeps_the_connection_and_next_stream_usable(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), async {
        let pair = MemoryPair::new().await?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let cancel = CancellationToken::new();
        let connection = pair
            .sender
            .request(pair.receiver_id)
            .connect(deadline, &cancel)
            .await?;
        let first = connection.open_bi(deadline).await?;
        let remote = pair.at_receiver.accept_bi().await?;
        first.stop()?;
        remote.stop()?;
        assert!(!connection.is_closed() && !pair.at_receiver.is_closed());
        let mut next = connection.open_bi(deadline).await?;
        let mut remote = pair.at_receiver.accept_bi().await?;
        next.send.write_all(b"second independent exchange").await?;
        next.send.finish()?;
        let mut bytes = Vec::new();
        remote.receive.read_to_end(&mut bytes).await?;
        assert_eq!(bytes, b"second independent exchange");
        remote.send.write_all(b"finished").await?;
        remote.send.finish()?;
        let mut bytes = Vec::new();
        next.receive.read_to_end(&mut bytes).await?;
        assert_eq!(bytes, b"finished");
        assert_eq!(pair.sender_dials.load(Ordering::SeqCst), 1);
        drop(next);
        drop(remote);
        drop(connection);
        pair.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
