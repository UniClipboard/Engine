//! 生产Router、原生handler与Application完整入站负责人；不在测试里逐流编排服务端。
#[path = "stream_reconnect_budget_probe.rs"]
mod reconnect_budget;
use super::{budget, descriptor, endpoint, item, manager, Authority, Directory, BUDGET, SOURCE};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::connectivity::{
    ConnectivityProfile, IrohConnectionAdmission, IrohConnectivityBackend,
    IrohTransferProgressHandler,
};
use crate::network::iroh::protocol_router::ProtocolRouterBuilder;
use crate::network::iroh::transfer_progress_v2::{
    TransferProgressInboundAdapter, TransferProgressWire,
};
use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::Endpoint;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    transfer_progress_connectivity_policy, ConnectionProtocol, ConnectivityError,
    ConnectivityManager, CurrentProgressTaskPort, ElapsedClockError, ElapsedClockPort,
    ProgressBound, ProgressContactNotice, ProgressPageReply, ProgressPageRequest,
    ProgressProjectionKey, ProgressProjectionRegistry, ProgressReceiveError, ProgressReceiverPort,
    ProgressSnapshotPage, ProgressSnapshotProviderPort, ProgressSnapshotQuery,
    ProgressSnapshotWirePort, ProgressTerminalNotice, ProgressUpdateNotice, ProgressWirePort,
    TransferProgressProducer, TransferProgressTransport,
};
use uc_core::file_transfer::{
    OutboundProgressStatus, ProgressFreshnessView, ProgressIdentity, ProgressItemState,
    ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN;
use uc_sync_protocol::transfer_progress::{
    write_frame, ProgressFrame, LEGACY_TRANSFER_PROGRESS_ALPN,
};
use uuid::Uuid;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
struct Admission {
    directory: Arc<Directory>,
    paused: AtomicBool,
    denied: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait]
impl PeerAdmissionPort for Admission {
    async fn is_admitted(&self, peer: &DeviceId) -> Result<bool, PeerAdmissionError> {
        if self.paused.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        if self.denied.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.directory.is_admitted(peer).await
    }
}
struct Adoption {
    registry: ProgressProjectionRegistry,
    calls: AtomicUsize,
    lose_ack: AtomicBool,
    paused: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait]
impl ProgressReceiverPort for Adoption {
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.registry.adopt_contact(peer, generation, notice).await
    }
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.registry.adopt_update(peer, generation, notice).await
    }
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        self.registry
            .register(peer, generation, source, exchange)
            .await
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.paused.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        let adopted = self
            .registry
            .adopt_terminal(peer, generation, notice)
            .await?;
        if self.lose_ack.load(Ordering::SeqCst) {
            // 真实采用之后结束原流，不向发送者交付ACK；原完整补发必须保留同一结果。
            return Err(ConnectivityError::Offline.into());
        }
        Ok(adopted)
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
struct Facts {
    peer: DeviceId,
}
#[async_trait]
impl ProgressSnapshotProviderPort for Facts {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        assert_eq!(*peer, self.peer);
        assert_eq!(query.source, SOURCE);
        assert_eq!(query.offset, 0);
        Ok(ProgressPageReply::Current(ProgressSnapshotPage {
            descriptor: descriptor(),
            revision: 7,
            offset: 0,
            total: 1,
            items: vec![ProgressItemState::Terminal(result())],
        }))
    }
}
#[derive(Clone)]
struct ObserveCompletion {
    inner: IrohTransferProgressHandler,
    done: mpsc::Sender<()>,
}
impl std::fmt::Debug for ObserveCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObserveProgressCompletion")
    }
}
impl ProtocolHandler for ObserveCompletion {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let result = self.inner.accept(connection).await;
        let _ = self.done.send(()).await;
        result
    }
    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}
/// 投影运行期独立于可重建的连接负责人；所有实际worker仍由测试明确等待。
struct ProjectionContext {
    registry: ProgressProjectionRegistry,
    stop: CancellationToken,
    worker: tokio::task::JoinHandle<()>,
}
impl ProjectionContext {
    fn new() -> Self {
        let (registry, worker) = ProgressProjectionRegistry::new(Arc::new(SystemElapsedClock));
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        Self {
            registry,
            stop,
            worker: running,
        }
    }
    async fn close(self) -> anyhow::Result<()> {
        self.stop.cancel();
        self.worker.await?;
        Ok(())
    }
}
struct Fixture {
    client: Arc<Endpoint>,
    router: Router,
    outbound: ConnectivityManager,
    owner: Arc<TransferProgressTransport>,
    registry: ProgressProjectionRegistry,
    adoption: Arc<Adoption>,
    admission: Arc<Admission>,
    clock: Arc<Clock>,
    peer: DeviceId,
    key: ProgressProjectionKey,
    done: mpsc::Receiver<()>,
}
impl Fixture {
    async fn new(legacy: bool, registry: ProgressProjectionRegistry) -> anyhow::Result<Self> {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("progress-source");
        let caller = DeviceId::new("progress-receiver");
        let directory = Arc::new(Directory {
            peers: vec![(peer, server.addr()), (caller, client.addr())],
            allowed: AtomicBool::new(true),
        });
        let outbound = manager(
            client.clone(),
            directory.clone(),
            AdmittedConnections::default(),
        );
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let admission = Arc::new(Admission {
            directory: directory.clone(),
            paused: AtomicBool::new(false),
            denied: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let inbound = ConnectivityManager::new(
            Arc::new(IrohConnectivityBackend::new(
                server.clone(),
                directory.clone(),
                None,
                ConnectivityProfile::TransferProgress,
            )),
            Arc::new(IrohConnectionAdmission::new(
                directory,
                admission.clone(),
                Arc::new(Sha256IdentityFingerprintFactory),
                AdmittedConnections::default(),
                ConnectivityProfile::TransferProgress,
            )),
            transfer_progress_connectivity_policy(clock.clone()),
        );
        let key = ProgressProjectionKey {
            peer: caller,
            source: SOURCE,
        };
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string(),
            ))
            .accept(key, vec![item()])?;
        let adoption = Arc::new(Adoption {
            registry: registry.clone(),
            calls: AtomicUsize::new(0),
            lose_ack: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let owner = registry.inbound_transport(
            inbound,
            Arc::new(TransferProgressInboundAdapter::new(
                adoption.clone(),
                Arc::new(registry.clone()),
                Arc::new(Facts { peer: caller }),
            )),
            clock.clone(),
        );
        let (done, finished) = mpsc::channel(8);
        let handler = ObserveCompletion {
            inner: IrohTransferProgressHandler::new(owner.clone()),
            done,
        };
        let mut builder = ProtocolRouterBuilder::new((*server).clone())
            .accept(LEGACY_TRANSFER_PROGRESS_ALPN, handler.clone());
        if !legacy {
            builder = builder.accept(TRANSFER_PROGRESS_ALPN, handler);
        }
        let router = builder.spawn();
        Ok(Self {
            client,
            router,
            outbound,
            owner,
            registry,
            adoption,
            admission,
            clock,
            peer,
            key,
            done: finished,
        })
    }
    fn producer(&self) -> TransferProgressProducer {
        TransferProgressProducer::new(
            self.outbound.clone(),
            Arc::new(TransferProgressWire),
            Arc::new(Authority),
            Arc::new(Facts { peer: self.peer }),
            Arc::new(SystemElapsedClock),
        )
    }
    async fn terminal(
        &self,
        bound: ProgressBound,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        let cancel = CancellationToken::new();
        let deadline = budget();
        let lease = self
            .outbound
            .request(self.peer)
            .connect(deadline.instant(), &cancel)
            .await?;
        let mut stream = lease
            .open_bi_with_cancel(deadline.instant(), &cancel)
            .await?;
        TransferProgressWire
            .terminal(
                &mut stream,
                &lease,
                ProgressTerminalNotice {
                    binding: bound.binding,
                    source: SOURCE,
                    attempt: bound.descriptor.attempt,
                    revision: bound.descriptor.revision,
                    result: result(),
                },
                &deadline,
                &cancel,
            )
            .await
    }
    async fn close(self) -> anyhow::Result<()> {
        self.owner.shutdown().await?;
        self.outbound.shutdown().await?;
        self.router.shutdown().await?;
        self.client.close().await;
        Ok(())
    }
}
#[tokio::test]
async fn native_handler_registers_acknowledges_queries_and_retires_old_connection_binding(
) -> anyhow::Result<()> {
    let context = ProjectionContext::new();
    let mut pair = Fixture::new(false, context.registry.clone()).await?;
    let old = pair
        .producer()
        .register_current(&pair.peer, SOURCE, &CancellationToken::new())
        .await?;
    assert_eq!(pair.terminal(old).await?.result, result());
    assert_eq!(
        pair.registry.snapshot(pair.key).unwrap().freshness.unwrap(),
        ProgressFreshnessView::Settled
    );
    let lease = pair
        .outbound
        .request(pair.peer)
        .connect(Instant::now() + BUDGET, &CancellationToken::new())
        .await?;
    let mut stream = lease.open_bi(Instant::now() + BUDGET).await?;
    let reply = TransferProgressWire
        .read_page(
            &mut stream,
            &lease,
            ProgressPageRequest {
                challenge: ProgressIdentity::new([9; 16]),
                source: SOURCE,
                attempt: Some(descriptor().attempt),
                offset: 0,
                revision: None,
            },
            &budget(),
            &CancellationToken::new(),
        )
        .await?;
    let ProgressPageReply::Current(page) = reply else {
        panic!("current page required")
    };
    assert_eq!(page.items, vec![ProgressItemState::Terminal(result())]);
    drop(stream);
    lease.close();
    drop(lease);
    timeout(BUDGET, pair.done.recv()).await?.unwrap();
    let current = pair
        .producer()
        .register_current(&pair.peer, SOURCE, &CancellationToken::new())
        .await?;
    assert_ne!(current.binding, old.binding);
    let version = pair.registry.snapshot(pair.key).unwrap().version;
    assert!(matches!(
        pair.terminal(old).await,
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(pair.registry.snapshot(pair.key).unwrap().version, version);
    assert_eq!(pair.terminal(current).await?.result, result());
    pair.close().await?;
    context.close().await
}
#[tokio::test]
async fn native_legacy_handler_adopts_only_unconfirmed_intent_and_closes_single_exchange(
) -> anyhow::Result<()> {
    let context = ProjectionContext::new();
    let pair = Fixture::new(true, context.registry.clone()).await?;
    let lease = pair
        .outbound
        .request(pair.peer)
        .connect(Instant::now() + BUDGET, &CancellationToken::new())
        .await?;
    assert_eq!(lease.protocol(), ConnectionProtocol::TransferProgressV1);
    let mut stream = lease
        .open_uni(Instant::now() + BUDGET, &CancellationToken::new())
        .await?;
    write_frame(
        &mut stream.stream,
        &ProgressFrame {
            transfer_id_bytes: SOURCE.bytes(),
            bytes_transferred: 10,
            total_bytes: Some(10),
            status: OutboundProgressStatus::Completed,
        },
    )
    .await?;
    stream.stream.finish()?;
    timeout(BUDGET, lease.closed()).await?;
    let view = pair.registry.snapshot(pair.key).unwrap();
    assert_eq!(view.freshness.unwrap(), ProgressFreshnessView::Unconfirmed);
    assert!(view.task.is_none() && view.bound.is_none());
    assert!(view.items.iter().all(|(_, item)| item.is_none()));
    drop(stream);
    drop(lease);
    pair.close().await?;
    context.close().await
}
#[tokio::test]
async fn native_inbound_budget_includes_elapsed_time_while_current_admission_is_held(
) -> anyhow::Result<()> {
    let context = ProjectionContext::new();
    let pair = Arc::new(Fixture::new(false, context.registry.clone()).await?);
    let bound = pair
        .producer()
        .register_current(&pair.peer, SOURCE, &CancellationToken::new())
        .await?;
    pair.admission.paused.store(true, Ordering::SeqCst);
    let sender = pair.clone();
    let sending = tokio::spawn(async move { sender.terminal(bound).await });
    timeout(BUDGET, pair.admission.entered.acquire())
        .await?
        .unwrap()
        .forget();
    pair.clock.0.store(6, Ordering::SeqCst);
    pair.admission.release.add_permits(1);
    assert!(timeout(BUDGET, sending).await??.is_err());
    assert_eq!(pair.adoption.calls.load(Ordering::SeqCst), 0);
    assert_eq!(pair.terminal(bound).await?.result, result());
    assert_eq!(pair.adoption.calls.load(Ordering::SeqCst), 1);
    let pair = Arc::try_unwrap(pair).ok().expect("all workers returned");
    pair.close().await?;
    context.close().await
}
#[tokio::test]
async fn cancelled_shutdown_waiter_cannot_abort_actual_terminal_adoption() -> anyhow::Result<()> {
    let context = ProjectionContext::new();
    let pair = Arc::new(Fixture::new(false, context.registry.clone()).await?);
    let bound = pair
        .producer()
        .register_current(&pair.peer, SOURCE, &CancellationToken::new())
        .await?;
    pair.adoption.paused.store(true, Ordering::SeqCst);
    let sender = pair.clone();
    let sending = tokio::spawn(async move { sender.terminal(bound).await });
    timeout(BUDGET, pair.adoption.entered.acquire())
        .await?
        .unwrap()
        .forget();
    let owner = pair.owner.clone();
    let waiting = tokio::spawn(async move { owner.shutdown().await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!waiting.is_finished());
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    let owner = pair.owner.clone();
    let mut again = tokio::spawn(async move { owner.shutdown().await });
    assert!(timeout(Duration::from_millis(30), &mut again)
        .await
        .is_err());
    pair.adoption.release.add_permits(1);
    timeout(BUDGET, again).await???;
    assert!(timeout(BUDGET, sending).await??.is_err());
    assert_eq!(
        pair.registry.snapshot(pair.key).unwrap().freshness.unwrap(),
        ProgressFreshnessView::Settled
    );
    let pair = Arc::try_unwrap(pair).ok().expect("all workers returned");
    pair.close().await?;
    context.close().await
}

#[tokio::test]
async fn native_member_revocation_after_registration_prevents_adoption_without_fallback(
) -> anyhow::Result<()> {
    let context = ProjectionContext::new();
    let pair = Fixture::new(false, context.registry.clone()).await?;
    let bound = pair
        .producer()
        .register_current(&pair.peer, SOURCE, &CancellationToken::new())
        .await?;
    let version = pair.registry.snapshot(pair.key).unwrap().version;
    pair.admission.denied.store(true, Ordering::SeqCst);
    assert!(pair.terminal(bound).await.is_err());
    assert_eq!(pair.adoption.calls.load(Ordering::SeqCst), 0);
    let view = pair.registry.snapshot(pair.key).unwrap();
    assert_eq!(view.version, version);
    assert!(view.items.iter().all(|(_, value)| value.is_none()));
    pair.close().await?;
    context.close().await
}

#[tokio::test]
async fn reassembled_transport_preserves_projection_connection_generation_namespace(
) -> anyhow::Result<()> {
    let context = ProjectionContext::new();
    let mut first = Fixture::new(false, context.registry.clone()).await?;
    let initial = first
        .producer()
        .register_current(&first.peer, SOURCE, &CancellationToken::new())
        .await?;
    let lease = first
        .outbound
        .request(first.peer)
        .connect(Instant::now() + BUDGET, &CancellationToken::new())
        .await?;
    lease.close();
    drop(lease);
    timeout(BUDGET, first.done.recv()).await?.unwrap();
    let previous = first
        .producer()
        .register_current(&first.peer, SOURCE, &CancellationToken::new())
        .await?;
    assert_ne!(previous.binding, initial.binding);
    first.close().await?;
    let second = Fixture::new(false, context.registry.clone()).await?;
    let current = second
        .producer()
        .register_current(&second.peer, SOURCE, &CancellationToken::new())
        .await?;
    assert!(current.descriptor == previous.descriptor);
    assert_ne!(current.binding, previous.binding);
    assert!(matches!(
        second.terminal(previous).await,
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(second.terminal(current).await?.result, result());
    second.close().await?;
    context.close().await
}
