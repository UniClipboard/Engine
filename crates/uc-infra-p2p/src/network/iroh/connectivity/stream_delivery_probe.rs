//! 同一接收设备、两个真实已认证来源；一个终态采用卡住时另端仍获得精确ACK。
use super::progress_probe::{progress_budget, Fixture, UnusedLive};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use async_trait::async_trait;
use iroh::{Endpoint, RelayMode};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Notify, Semaphore};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, CurrentProgressTaskPort, ProgressBound, ProgressClaimRefusal,
    ProgressExchangeDeadline, ProgressExchangeService, ProgressReceiveError, ProgressReceiverPort,
    ProgressSourceTask, ProgressSubmission, ProgressTaskAuthorityPort, ProgressTaskDescriptor,
    ProgressTerminalNotice, ProgressTerminalRoundPort, ProgressUpdateNotice,
    TransferProgressProducer, TransferProgressReceiver, TransferProgressSnapshotReader,
};
use uc_core::file_transfer::ProgressItemState;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN;

const SOURCE: ProgressIdentity = ProgressIdentity::new([1; 16]);
const ATTEMPT: ProgressIdentity = ProgressIdentity::new([3; 16]);
fn item() -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index: 0,
    }
}
fn result() -> ProgressTerminal {
    ProgressTerminal {
        item: item(),
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 10,
        total: Some(10),
        outcome: ProgressTerminalOutcome::Completed,
    }
}
struct Authority {
    tasks: HashMap<DeviceId, ProgressSourceTask>,
    claimed: Mutex<HashSet<DeviceId>>,
}
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        if source != SOURCE {
            return None;
        }
        self.tasks.get(peer).cloned().map(|mut task| {
            if self.claimed.lock().unwrap().contains(peer) {
                task.notification_closed = Arc::new(task.items.iter().copied().collect());
            }
            task
        })
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        logical: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        let task = self
            .current_task(peer, source)
            .ok_or(ProgressClaimRefusal::NotRetained)?;
        if attempt != task.descriptor.attempt {
            return Err(ProgressClaimRefusal::Superseded);
        }
        if !task.items.contains(&logical) {
            return Err(ProgressClaimRefusal::InvalidItem);
        }
        if !self.claimed.lock().unwrap().insert(*peer) {
            return Err(ProgressClaimRefusal::AlreadyObserved);
        }
        Ok(task)
    }
}
struct Paused {
    owner: Arc<TransferProgressReceiver>,
    entered: Notify,
    release: Semaphore,
}
#[async_trait]
impl ProgressReceiverPort for Paused {
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.owner.adopt_update(peer, generation, notice).await
    }
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        self.owner
            .register(peer, generation, source, exchange)
            .await
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        self.owner.adopt_terminal(peer, generation, notice).await
    }
}
struct Rounds {
    clients: HashMap<DeviceId, TransferProgressProducer>,
    events: mpsc::Sender<(DeviceId, bool)>,
    active: AtomicUsize,
    peak: AtomicUsize,
    peers: Mutex<HashSet<DeviceId>>,
}
#[async_trait]
impl ProgressTerminalRoundPort for Rounds {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        assert!(self.peers.lock().unwrap().insert(*peer));
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        assert!(active <= 4);
        let reported = self.clients[peer]
            .report_settled(peer, expected, result, deadline, cancel)
            .await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.peers.lock().unwrap().remove(peer);
        self.events.try_send((*peer, reported.is_ok())).unwrap();
        reported
    }
}
async fn serve_pair(
    connection: ConnectionLease,
    owner: Arc<dyn ProgressReceiverPort>,
    stop: CancellationToken,
) -> Result<(), ProgressReceiveError> {
    for _ in 0..2 {
        TransferProgressInbound::serve(
            connection.accept_bi().await?,
            &connection,
            1,
            progress_budget(),
            &stop,
            owner.as_ref(),
        )
        .await?;
    }
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_authenticated_peer_does_not_hold_up_another_peers_actual_terminal_ack(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let a = DeviceId::new("progress-source-a");
        let b = DeviceId::new("progress-source-b");
        let first = Fixture::new_as(a, 4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![TRANSFER_PROGRESS_ALPN.to_vec()])
            .bind()
            .await?;
        // 共用同一个本机Endpoint身份，远端是两个不同已认证身份与业务设备。
        let second = Fixture::connect_as(
            first.client.clone(),
            server,
            4,
            4,
            TRANSFER_PROGRESS_ALPN,
            b,
        )
        .await?;
        let task = ProgressSourceTask {
            notification_closed: Default::default(),
            descriptor: ProgressTaskDescriptor {
                producer: ProgressIdentity::new([2; 16]),
                revision: 1,
                source: SOURCE,
                attempt: ATTEMPT,
                item_count: 1,
            },
            items: Arc::from([item()]),
        };
        let authority = Arc::new(Authority {
            tasks: HashMap::from([(a, task.clone()), (b, task)]),
            claimed: Mutex::new(HashSet::new()),
        });
        let paused = Arc::new(Paused {
            owner: Arc::new(TransferProgressReceiver::new(
                first.receiving.peer(),
                SOURCE,
                vec![item()],
            )?),
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        let fast = Arc::new(TransferProgressReceiver::new(
            second.receiving.peer(),
            SOURCE,
            vec![item()],
        )?);
        let server_stop = CancellationToken::new();
        let slow_task = tokio::spawn(serve_pair(
            first.receiving.clone(),
            paused.clone(),
            server_stop.clone(),
        ));
        let fast_task = tokio::spawn(serve_pair(
            second.receiving.clone(),
            fast.clone(),
            server_stop.clone(),
        ));
        let (tx, mut events) = mpsc::channel(16);
        let rounds = Arc::new(Rounds {
            clients: HashMap::from([
                (
                    a,
                    TransferProgressProducer::new(
                        first.outbound.clone(),
                        Arc::new(TransferProgressWire),
                        authority.clone(),
                        Arc::new(SystemElapsedClock),
                    ),
                ),
                (
                    b,
                    TransferProgressProducer::new(
                        second.outbound.clone(),
                        Arc::new(TransferProgressWire),
                        authority.clone(),
                        Arc::new(SystemElapsedClock),
                    ),
                ),
            ]),
            events: tx,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            peers: Mutex::new(HashSet::new()),
        });
        let (delivery, worker) = ProgressExchangeService::new(
            Arc::new(SystemElapsedClock),
            authority,
            rounds.clone(),
            Arc::new(TransferProgressSnapshotReader::new(
                first.outbound.clone(),
                Arc::new(TransferProgressWire),
            )),
            Arc::new(UnusedLive),
        );
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        assert_eq!(
            delivery.submit_settled(a, SOURCE, ATTEMPT, result())?,
            ProgressSubmission::Retained
        );
        timeout(Duration::from_secs(5), paused.entered.notified()).await?;
        assert_eq!(
            delivery.submit_settled(b, SOURCE, ATTEMPT, result())?,
            ProgressSubmission::Retained
        );
        assert_eq!(
            timeout(Duration::from_secs(3), events.recv()).await?,
            Some((b, true))
        );
        assert_eq!(
            fast.snapshot().items[0].1,
            Some(ProgressItemState::Terminal(result()))
        );
        assert!(paused.owner.snapshot().items[0].1.is_none());
        assert!(rounds.peak.load(Ordering::SeqCst) <= 4);
        stop.cancel();
        timeout(Duration::from_secs(5), running).await??;
        assert_eq!(rounds.active.load(Ordering::SeqCst), 0);
        // 远端已开始的采用仍有自己的真实监督，不因出站等待取消而丢弃。
        assert!(!slow_task.is_finished());
        server_stop.cancel();
        paused.release.add_permits(1);
        assert!(timeout(Duration::from_secs(5), slow_task).await??.is_err());
        timeout(Duration::from_secs(5), fast_task).await???;
        assert_eq!(
            paused.owner.snapshot().items[0].1,
            Some(ProgressItemState::Terminal(result()))
        );
        first.close().await?;
        second.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
