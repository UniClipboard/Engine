//! 专属profile经实际Iroh backend拨号、真实身份准入及跨请求保留，不借用预连backend。
use super::super::{
    ConnectivityProfile, IrohConnectionAdmission, IrohConnectivityBackend, IrohPeerConnection,
};
use super::progress_probe::UnusedSnapshotProvider;
use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    transfer_progress_connectivity_policy, ConnectionLease, ConnectionProtocol, ConnectivityError,
    ConnectivityManager, ElapsedClockPort, KnownPeerIdentity, MembershipLedgerError,
    PeerConnection, PeerIdentityDirectoryPort, ProgressClaimRefusal, ProgressExchangeDeadline,
    ProgressProjectionKey, ProgressProjectionRegistry, ProgressSourceTask,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalRoundPort,
    TransferProgressProducer,
};
use uc_core::file_transfer::{
    ProgressFreshnessView, ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN;
use uc_sync_protocol::transfer_progress::LEGACY_TRANSFER_PROGRESS_ALPN;

const BUDGET: Duration = Duration::from_secs(5);
const SOURCE: ProgressIdentity = ProgressIdentity::new([1; 16]);
struct Directory {
    peers: Vec<(DeviceId, EndpointAddr)>,
    allowed: AtomicBool,
}
#[async_trait]
impl PeerAddressRepositoryPort for Directory {
    async fn get(&self, peer: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(self
            .peers
            .iter()
            .find(|(id, _)| id == peer)
            .map(|(_, addr)| PeerAddressRecord {
                device_id: *peer,
                addr_blob: postcard::to_stdvec(addr).unwrap(),
                observed_at: chrono::Utc::now(),
            }))
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(Vec::new())
    }
    async fn upsert(&self, _: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        panic!("read-only profile fixture")
    }
    async fn remove(&self, _: &DeviceId) -> Result<(), PeerAddressError> {
        panic!("read-only profile fixture")
    }
}
#[async_trait]
impl PeerIdentityDirectoryPort for Directory {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(self
            .peers
            .iter()
            .map(|(peer, addr)| KnownPeerIdentity {
                device_id: *peer,
                identity_fingerprint: Sha256IdentityFingerprintFactory
                    .from_public_key(addr.id.as_bytes())
                    .unwrap(),
            })
            .collect())
    }
}
#[async_trait]
impl PeerAdmissionPort for Directory {
    async fn is_admitted(&self, peer: &DeviceId) -> Result<bool, PeerAdmissionError> {
        Ok(self.allowed.load(Ordering::SeqCst) && self.peers.iter().any(|(id, _)| id == peer))
    }
}
async fn endpoint() -> anyhow::Result<Arc<Endpoint>> {
    let endpoint = Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?,
    );
    timeout(BUDGET, async {
        while endpoint.addr().ip_addrs().next().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(endpoint)
}
fn manager(
    endpoint: Arc<Endpoint>,
    directory: Arc<Directory>,
    ledger: AdmittedConnections,
) -> ConnectivityManager {
    ConnectivityManager::new(
        Arc::new(IrohConnectivityBackend::new(
            endpoint,
            directory.clone(),
            None,
            ConnectivityProfile::TransferProgress,
        )),
        Arc::new(IrohConnectionAdmission::new(
            directory.clone(),
            directory,
            Arc::new(Sha256IdentityFingerprintFactory),
            ledger,
            ConnectivityProfile::TransferProgress,
        )),
        transfer_progress_connectivity_policy(Arc::new(SystemElapsedClock)),
    )
}
type Incoming = (ConnectionLease, Arc<dyn PeerConnection>);
#[derive(Clone)]
struct Capture {
    manager: ConnectivityManager,
    events: mpsc::Sender<Incoming>,
}
impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressProfileCapture")
    }
}
impl ProtocolHandler for Capture {
    async fn accept(&self, native: Connection) -> Result<(), AcceptError> {
        let raw = IrohPeerConnection::wrap(native);
        if let Ok(lease) = self.manager.accept(raw.clone()).await {
            self.manager.retain(&lease).await.unwrap();
            self.events.send((lease, raw.clone())).await.unwrap();
            raw.closed().await;
        }
        Ok(())
    }
}
fn descriptor() -> ProgressTaskDescriptor {
    ProgressTaskDescriptor {
        producer: ProgressIdentity::new([2; 16]),
        revision: 1,
        source: SOURCE,
        attempt: ProgressIdentity::new([3; 16]),
        item_count: 1,
    }
}
fn item() -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index: 0,
    }
}
struct Authority;
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(&self, _: &DeviceId, source: ProgressIdentity) -> Option<ProgressSourceTask> {
        (source == SOURCE).then(|| ProgressSourceTask {
            descriptor: descriptor(),
            items: Arc::from([item()]),
            notification_closed: Default::default(),
        })
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressIdentity,
        _: ProgressIdentity,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("one round fixture does not submit a new intent")
    }
}
fn budget() -> ProgressExchangeDeadline {
    ProgressExchangeDeadline::new(
        Arc::new(SystemElapsedClock),
        SystemElapsedClock.elapsed().unwrap() + BUDGET,
    )
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_backend_progress_profile_adopts_terminal_and_reuses_current_retained_connection(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("server");
        let sender = DeviceId::new("client");
        let directory = Arc::new(Directory {
            peers: vec![(peer, server.addr()), (sender, client.addr())],
            allowed: AtomicBool::new(true),
        });
        let outbound = manager(
            client.clone(),
            directory.clone(),
            AdmittedConnections::default(),
        );
        let inbound = manager(server.clone(), directory, AdmittedConnections::default());
        let (tx, mut rx) = mpsc::channel(8);
        let router = Router::builder((*server).clone())
            .accept(
                TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx.clone(),
                },
            )
            // TLS按服务端注册顺序选择；必须先/2再/1。
            .accept(
                LEGACY_TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx,
                },
            )
            .spawn();
        let (registry, worker) = ProgressProjectionRegistry::new(Arc::new(SystemElapsedClock));
        let key = ProgressProjectionKey {
            peer: sender,
            source: SOURCE,
        };
        registry.accept_intent(key, vec![item()])?;
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        let producer = TransferProgressProducer::new(
            outbound.clone(),
            Arc::new(TransferProgressWire),
            Arc::new(Authority),
            Arc::new(UnusedSnapshotProvider),
            Arc::new(SystemElapsedClock),
        );
        let result = ProgressTerminal {
            item: item(),
            round: 0,
            sequence: 1,
            result_revision: 1,
            bytes: 10,
            total: Some(10),
            outcome: ProgressTerminalOutcome::Completed,
        };
        let serve = async {
            let (lease, raw) = rx.recv().await.unwrap();
            assert_eq!(lease.protocol(), ConnectionProtocol::TransferProgressV2);
            for _ in 0..2 {
                TransferProgressInbound::serve(
                    lease.accept_bi().await?,
                    &lease,
                    1,
                    budget(),
                    &stop,
                    &registry,
                    &UnusedSnapshotProvider,
                )
                .await?;
            }
            assert!(inbound.retain(&lease).await?);
            anyhow::Ok(raw)
        };
        let deadline = budget();
        let (sent, served) = tokio::join!(
            producer.report_settled(&peer, descriptor(), result, &deadline, &stop),
            serve
        );
        sent?;
        let raw = served?;
        assert_eq!(
            registry.snapshot(key).unwrap().freshness.unwrap(),
            ProgressFreshnessView::Settled
        );
        let first = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let id = first.connection_id();
        assert!(outbound.retain(&first).await?);
        drop(first);
        let next = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        assert_eq!(next.connection_id(), id);
        assert!(rx.try_recv().is_err());
        assert!(!raw.is_closed());
        drop(next);
        stop.cancel();
        running.await?;
        outbound.shutdown().await?;
        inbound.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[path = "stream_profile_limits_probe.rs"]
mod limits_probe;

/// 最大实际CAS分页组合复用同一个真实backend/专属profile，不回到预连Clipboard fixture。
pub(super) struct ProfilePair {
    pub outbound: ConnectivityManager,
    pub receiving: ConnectionLease,
    pub peer: DeviceId,
    inbound: ConnectivityManager,
    router: Router,
    client: Arc<Endpoint>,
}
impl ProfilePair {
    pub async fn new() -> anyhow::Result<Self> {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("progress-receiver");
        let sender = DeviceId::new("progress-source");
        let directory = Arc::new(Directory {
            peers: vec![(peer, server.addr()), (sender, client.addr())],
            allowed: AtomicBool::new(true),
        });
        let outbound = manager(
            client.clone(),
            directory.clone(),
            AdmittedConnections::default(),
        );
        let inbound = manager(server.clone(), directory, AdmittedConnections::default());
        let (tx, mut rx) = mpsc::channel(4);
        let router = Router::builder((*server).clone())
            .accept(
                TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx,
                },
            )
            .spawn();
        let sending = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &CancellationToken::new())
            .await?;
        assert_eq!(sending.protocol(), ConnectionProtocol::TransferProgressV2);
        let (receiving, _) = timeout(BUDGET, rx.recv()).await?.unwrap();
        assert!(outbound.retain(&sending).await?);
        drop(sending);
        Ok(Self {
            outbound,
            receiving,
            peer,
            inbound,
            router,
            client,
        })
    }
    pub async fn close(self) -> anyhow::Result<()> {
        drop(self.receiving);
        self.outbound.shutdown().await?;
        self.inbound.shutdown().await?;
        self.router.shutdown().await?;
        self.client.close().await;
        Ok(())
    }
}

#[path = "stream_profile_legacy_summary_probe.rs"]
mod legacy_summary_probe;

pub(super) use legacy_summary_probe::LegacyPair;
