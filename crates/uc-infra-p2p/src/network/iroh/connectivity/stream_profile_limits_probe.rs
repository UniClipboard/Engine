use super::super::super::BranchRecoveryAdmission;
use super::*;
use iroh::endpoint::WeakConnectionHandle;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize},
    Mutex,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use uc_application::deps::{
    BranchRecoveryPeerPort, ConnectivityBackend, ConnectivityPolicy, StreamLease,
};
use uc_application::deps::{ElapsedClockError, ElapsedClockPort};
use uc_core::file_transfer::OutboundProgressStatus;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::clipboard::MULTIPLEXED_CLIPBOARD_ALPN;
use uc_sync_protocol::membership_branch_recovery::MEMBERSHIP_BRANCH_RECOVERY_ALPN;
use uc_sync_protocol::transfer_progress::{self, ProgressFrame, LEGACY_TRANSFER_PROGRESS_ALPN};

#[cfg(unix)]
#[path = "stream_profile_cross_process_probe.rs"]
mod cross_process;

async fn incoming(rx: &mut mpsc::Receiver<Incoming>) -> Incoming {
    timeout(BUDGET, rx.recv()).await.unwrap().unwrap()
}

#[async_trait]
impl BranchRecoveryPeerPort for Directory {
    async fn is_current_recovery_peer(
        &self,
        peer: &DeviceId,
    ) -> Result<bool, MembershipLedgerError> {
        Ok(self.allowed.load(Ordering::SeqCst) && self.peers.iter().any(|(id, _)| id == peer))
    }
}

type RecoveryIncoming =
    Result<(Arc<dyn PeerConnection>, ConnectionLease, StreamLease), ConnectivityError>;

#[derive(Clone)]
struct RecoveryCapacityCapture {
    manager: ConnectivityManager,
    events: mpsc::Sender<RecoveryIncoming>,
    active: Arc<AtomicUsize>,
    native_handles: Arc<Mutex<Vec<WeakConnectionHandle>>>,
}

struct ActiveRecoveryHandler(Arc<AtomicUsize>);

impl Drop for ActiveRecoveryHandler {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for RecoveryCapacityCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RecoveryCapacityCapture")
    }
}

impl ProtocolHandler for RecoveryCapacityCapture {
    async fn accept(&self, native: Connection) -> Result<(), AcceptError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveRecoveryHandler(self.active.clone());
        self.native_handles
            .lock()
            .unwrap()
            .push(native.weak_handle());
        let raw = IrohPeerConnection::wrap(native);
        let result = self
            .manager
            .accept_exchange(raw.clone(), Instant::now() + Duration::from_secs(10))
            .await
            .map(|(lease, stream)| (raw.clone(), lease, stream));
        if self.events.send(result).await.is_ok() {
            raw.closed().await;
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_recovery_connections_enforce_inbound_capacity_and_shutdown() -> anyhow::Result<()> {
    timeout(Duration::from_secs(60), async {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let directory = Arc::new(Directory {
            peers: vec![
                (DeviceId::new("client"), client.addr()),
                (DeviceId::new("server"), server.addr()),
            ],
            allowed: AtomicBool::new(true),
        });
        let manager = ConnectivityManager::new(
            Arc::new(IrohConnectivityBackend::new(
                server.clone(),
                directory.clone(),
                None,
                ConnectivityProfile::MembershipBranchRecovery,
            )),
            Arc::new(BranchRecoveryAdmission::new(
                directory.clone(),
                directory,
                Arc::new(Sha256IdentityFingerprintFactory),
                AdmittedConnections::default(),
            )),
            ConnectivityPolicy {
                outbound_per_peer: 1,
                inbound_per_connection: 1,
                inbound_per_session: 8,
                reject_excess_inbound: true,
                waiters_per_peer: 4,
                waiters_per_session: 16,
                flights_per_session: 4,
                supervise_local_work: true,
                ..ConnectivityPolicy::default()
            },
        );
        let (events, mut received) = mpsc::channel(16);
        let handlers = Arc::new(AtomicUsize::new(0));
        let server_handles = Arc::new(Mutex::new(Vec::new()));
        let router = Router::builder((*server).clone())
            .accept(
                MEMBERSHIP_BRANCH_RECOVERY_ALPN,
                RecoveryCapacityCapture {
                    manager: manager.clone(),
                    events,
                    active: handlers.clone(),
                    native_handles: server_handles.clone(),
                },
            )
            .spawn();
        let mut clients = Vec::new();
        let mut active = Vec::new();
        let mut native_handles = Vec::new();
        for _ in 0..8 {
            let connection = client
                .connect(server.addr(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
                .await?;
            native_handles.push(connection.weak_handle());
            let (mut send, receive) = connection.open_bi().await?;
            send.write_all(&[1]).await?;
            let accepted = timeout(BUDGET, received.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("missing inbound event"))??;
            assert_eq!(
                accepted.1.protocol(),
                ConnectionProtocol::MembershipBranchRecovery
            );
            clients.push((connection, send, receive));
            active.push(accepted);
        }
        let rejected = client
            .connect(server.addr(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
            .await?;
        native_handles.push(rejected.weak_handle());
        let result = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing overflow event"))?;
        assert!(matches!(result, Err(ConnectivityError::Busy(_))));
        timeout(BUDGET, rejected.closed()).await?;
        timeout(BUDGET, async {
            while handlers.load(Ordering::SeqCst) != 8 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(active.pop());
        timeout(BUDGET, async {
            while handlers.load(Ordering::SeqCst) != 7 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let replacement = client
            .connect(server.addr(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
            .await?;
        native_handles.push(replacement.weak_handle());
        let (mut send, receive) = replacement.open_bi().await?;
        send.write_all(&[1]).await?;
        let accepted = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing replacement event"))??;
        clients.push((replacement, send, receive));
        active.push(accepted);
        manager.shutdown().await?;
        for (raw, _, _) in &active {
            timeout(BUDGET, raw.closed()).await?;
        }
        timeout(BUDGET, async {
            while handlers.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(active);
        drop(clients);
        drop(rejected);
        router.shutdown().await?;
        client.close().await;
        assert_eq!(native_handles.len(), 10);
        assert_eq!(server_handles.lock().unwrap().len(), 10);
        timeout(BUDGET, async {
            while native_handles
                .iter()
                .any(|handle| handle.upgrade().is_some())
                || server_handles
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|handle| handle.upgrade().is_some())
            {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_recovery_identity_read_keeps_original_revocation_fence() -> anyhow::Result<()> {
    struct DelayedDirectory {
        directory: Arc<Directory>,
        first: AtomicBool,
        started: Semaphore,
        release: Semaphore,
    }

    #[async_trait]
    impl PeerIdentityDirectoryPort for DelayedDirectory {
        async fn known_peer_identities(
            &self,
        ) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
            if self.first.swap(false, Ordering::SeqCst) {
                self.started.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            self.directory.known_peer_identities().await
        }
    }

    timeout(Duration::from_secs(30), async {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("client");
        let directory = Arc::new(Directory {
            peers: vec![
                (peer, client.addr()),
                (DeviceId::new("server"), server.addr()),
            ],
            allowed: AtomicBool::new(true),
        });
        let delayed = Arc::new(DelayedDirectory {
            directory: directory.clone(),
            first: AtomicBool::new(true),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let ledger = AdmittedConnections::default();
        let manager = ConnectivityManager::new(
            Arc::new(IrohConnectivityBackend::new(
                server.clone(),
                directory.clone(),
                None,
                ConnectivityProfile::MembershipBranchRecovery,
            )),
            Arc::new(BranchRecoveryAdmission::new(
                delayed.clone(),
                directory.clone(),
                Arc::new(Sha256IdentityFingerprintFactory),
                ledger.clone(),
            )),
            ConnectivityPolicy {
                inbound_per_session: 8,
                reject_excess_inbound: true,
                supervise_local_work: true,
                ..ConnectivityPolicy::default()
            },
        );
        let (events, mut received) = mpsc::channel(2);
        let router = Router::builder((*server).clone())
            .accept(
                MEMBERSHIP_BRANCH_RECOVERY_ALPN,
                RecoveryCapacityCapture {
                    manager: manager.clone(),
                    events,
                    active: Arc::new(AtomicUsize::new(0)),
                    native_handles: Arc::new(Mutex::new(Vec::new())),
                },
            )
            .spawn();
        let old = client
            .connect(server.addr(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
            .await?;
        timeout(BUDGET, delayed.started.acquire()).await??.forget();
        directory.allowed.store(false, Ordering::SeqCst);
        ledger.close_device(&peer);
        directory.allowed.store(true, Ordering::SeqCst);
        delayed.release.add_permits(1);
        let old_result = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing old admission"))?;
        assert!(matches!(old_result, Err(ConnectivityError::Denied)));
        timeout(BUDGET, old.closed()).await?;

        let fresh = client
            .connect(server.addr(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
            .await?;
        let (mut send, _receive) = fresh.open_bi().await?;
        send.write_all(&[1]).await?;
        let (_raw, lease, stream) = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing fresh admission"))??;
        assert_eq!(
            lease.protocol(),
            ConnectionProtocol::MembershipBranchRecovery
        );
        drop(stream);
        drop(lease);
        manager.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_recovery_dial_does_not_adopt_connection_after_rejoin() -> anyhow::Result<()> {
    fn denied(error: &ConnectivityError) -> bool {
        match error {
            ConnectivityError::Denied => true,
            ConnectivityError::Shared(source) => denied(source),
            _ => false,
        }
    }

    struct DelayedBackend {
        inner: IrohConnectivityBackend,
        first: AtomicBool,
        started: Semaphore,
        release: Semaphore,
    }

    #[async_trait]
    impl ConnectivityBackend for DelayedBackend {
        async fn connect(
            &self,
            target: DeviceId,
            hint: Option<Vec<u8>>,
            cancel: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            let connection = self.inner.connect(target, hint, cancel).await?;
            if self.first.swap(false, Ordering::SeqCst) {
                self.started.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            Ok(connection)
        }
    }

    #[derive(Clone)]
    struct RawCapture(mpsc::Sender<Connection>);
    impl std::fmt::Debug for RawCapture {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("RawRecoveryCapture")
        }
    }
    impl ProtocolHandler for RawCapture {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            if self.0.send(connection.clone()).await.is_ok() {
                connection.closed().await;
            }
            Ok(())
        }
    }

    timeout(Duration::from_secs(30), async {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("server");
        let directory = Arc::new(Directory {
            peers: vec![
                (DeviceId::new("client"), client.addr()),
                (peer, server.addr()),
            ],
            allowed: AtomicBool::new(true),
        });
        let backend = Arc::new(DelayedBackend {
            inner: IrohConnectivityBackend::new(
                client.clone(),
                directory.clone(),
                None,
                ConnectivityProfile::MembershipBranchRecovery,
            ),
            first: AtomicBool::new(true),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let ledger = AdmittedConnections::default();
        let manager = ConnectivityManager::new(
            backend.clone(),
            Arc::new(BranchRecoveryAdmission::new(
                directory.clone(),
                directory.clone(),
                Arc::new(Sha256IdentityFingerprintFactory),
                ledger.clone(),
            )),
            ConnectivityPolicy {
                supervise_local_work: true,
                ..ConnectivityPolicy::default()
            },
        );
        let (events, mut received) = mpsc::channel(2);
        let router = Router::builder((*server).clone())
            .accept(MEMBERSHIP_BRANCH_RECOVERY_ALPN, RawCapture(events))
            .spawn();
        let request = manager.request(peer);
        let old = tokio::spawn(async move {
            request
                .connect(Instant::now() + BUDGET, &CancellationToken::new())
                .await
        });
        timeout(BUDGET, backend.started.acquire()).await??.forget();
        let old_remote = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing old connection"))?;
        directory.allowed.store(false, Ordering::SeqCst);
        ledger.close_device(&peer);
        directory.allowed.store(true, Ordering::SeqCst);
        backend.release.add_permits(1);
        let error = match old.await? {
            Ok(_) => anyhow::bail!("old request adopted a connection after rejoin"),
            Err(error) => error,
        };
        assert!(denied(&error), "旧拨号必须因撤销而拒绝: {error}");
        timeout(BUDGET, old_remote.closed()).await?;

        let fresh = manager
            .request(peer)
            .connect(Instant::now() + BUDGET, &CancellationToken::new())
            .await?;
        assert_eq!(
            fresh.protocol(),
            ConnectionProtocol::MembershipBranchRecovery
        );
        let fresh_remote = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing fresh connection"))?;
        fresh.close();
        timeout(BUDGET, fresh_remote.closed()).await?;
        drop(fresh);
        manager.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_work_leases_survive_thirty_seconds_then_real_idle_timer_closes_the_connection(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(90), async {
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
                    events: tx,
                },
            )
            .spawn();
        let stop = CancellationToken::new();
        let lease = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let (remote, raw) = incoming(&mut rx).await;
        let mut stream = lease.open_bi(Instant::now() + BUDGET).await?;
        stream.send.write_all(&[1]).await?;
        let mut accepted = remote.accept_bi().await?;
        let mut byte = [0];
        accepted.receive.read_exact(&mut byte).await?;
        assert!(outbound.retain(&lease).await?);
        let (io, work) = stream.into_parts();
        let (remote_io, remote_work) = accepted.into_parts();
        // 网络已停止，只保留实际本机工作的完整使用权；不能因五秒已过提前当作空闲。
        drop(io);
        drop(remote_io);
        drop(lease);
        drop(remote);
        tokio::time::sleep(Duration::from_secs(31)).await;
        assert!(!raw.is_closed());
        drop(work);
        drop(remote_work);
        let released = Instant::now();
        timeout(Duration::from_secs(35), raw.closed()).await?;
        assert!(released.elapsed() >= Duration::from_secs(29));
        outbound.shutdown().await?;
        inbound.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sixteen_retained_real_peers_evict_idle_only_and_all_active_capacity_refuses_retention(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(60), async {
        let client = endpoint().await?;
        let sender = DeviceId::new("client");
        let mut servers = Vec::new();
        let mut peers = vec![(sender, client.addr())];
        for n in 0..17 {
            let endpoint = endpoint().await?;
            let peer = DeviceId::new(format!("server-{n}"));
            peers.push((peer, endpoint.addr()));
            servers.push((peer, endpoint));
        }
        let directory = Arc::new(Directory {
            peers,
            allowed: AtomicBool::new(true),
        });
        let outbound = manager(
            client.clone(),
            directory.clone(),
            AdmittedConnections::default(),
        );
        let (tx, mut rx) = mpsc::channel(32);
        let mut owners = Vec::new();
        let mut routers = Vec::new();
        for (_, server) in &servers {
            let owner = manager(
                server.clone(),
                directory.clone(),
                AdmittedConnections::default(),
            );
            routers.push(
                Router::builder((**server).clone())
                    .accept(
                        TRANSFER_PROGRESS_ALPN,
                        Capture {
                            manager: owner.clone(),
                            events: tx.clone(),
                        },
                    )
                    .spawn(),
            );
            owners.push(owner);
        }
        let stop = CancellationToken::new();
        let mut observed = Vec::new();
        let mut ids = Vec::new();
        for (peer, _) in &servers {
            let lease = outbound
                .request(*peer)
                .connect(Instant::now() + BUDGET, &stop)
                .await?;
            assert!(outbound.retain(&lease).await?);
            ids.push(lease.connection_id());
            let (remote, raw) = incoming(&mut rx).await;
            drop(remote);
            observed.push(raw);
            drop(lease);
        }
        timeout(BUDGET, observed[0].closed()).await?;
        assert!(observed[1..].iter().all(|raw| !raw.is_closed()));
        let mut active = Vec::new();
        for (n, (peer, _)) in servers.iter().enumerate().skip(1) {
            let lease = outbound
                .request(*peer)
                .connect(Instant::now() + BUDGET, &stop)
                .await?;
            assert_eq!(lease.connection_id(), ids[n]);
            active.push(lease);
        }
        assert!(rx.try_recv().is_err());
        let overflow = outbound
            .request(servers[0].0)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let (remote, raw) = incoming(&mut rx).await;
        drop(remote);
        assert!(!outbound.retain(&overflow).await?);
        drop(overflow);
        timeout(BUDGET, raw.closed()).await?;
        assert!(observed[1..].iter().all(|raw| !raw.is_closed()));
        drop(active);
        outbound.shutdown().await?;
        for owner in owners {
            owner.shutdown().await?;
        }
        for router in routers {
            router.shutdown().await?;
        }
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_only_peer_negotiates_original_uni_frame_without_idle_retention(
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
                LEGACY_TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx,
                },
            )
            .spawn();
        let stop = CancellationToken::new();
        let lease = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let (remote, _) = incoming(&mut rx).await;
        assert_eq!(lease.protocol(), ConnectionProtocol::TransferProgressV1);
        assert!(!outbound.retain(&lease).await?);
        assert!(!inbound.retain(&remote).await?);
        let frame = ProgressFrame {
            transfer_id_bytes: [7; 16],
            bytes_transferred: 10,
            total_bytes: Some(10),
            status: OutboundProgressStatus::Completed,
        };
        let mut send = lease.open_uni(Instant::now() + BUDGET, &stop).await?;
        transfer_progress::write_frame(&mut send.stream, &frame).await?;
        send.stream.finish()?;
        let mut receive = remote.accept_uni(Instant::now() + BUDGET, &stop).await?;
        assert_eq!(
            transfer_progress::read_frame(&mut receive.stream).await?,
            frame
        );
        drop(send);
        drop(receive);
        drop(lease);
        drop(remote);
        outbound.shutdown().await?;
        inbound.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

struct HeldIdentities {
    directory: Arc<Directory>,
    release: Arc<Semaphore>,
}
#[async_trait]
impl PeerIdentityDirectoryPort for HeldIdentities {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        self.release.acquire().await.unwrap().forget();
        self.directory.known_peer_identities().await
    }
}
fn busy(source: &ConnectivityError) -> bool {
    match source {
        ConnectivityError::Busy(_) => true,
        ConnectivityError::Shared(source) => busy(source),
        _ => false,
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direction_capacity_and_progress_retirement_are_isolated_but_member_revocation_is_shared(
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
        let client_ledger = AdmittedConnections::default();
        let server_ledger = AdmittedConnections::default();
        let outbound = manager(client.clone(), directory.clone(), client_ledger.clone());
        let inbound = manager(server.clone(), directory.clone(), server_ledger.clone());
        let make_clip = |endpoint: Arc<Endpoint>, ledger: AdmittedConnections| {
            ConnectivityManager::new(
                Arc::new(IrohConnectivityBackend::new(
                    endpoint,
                    directory.clone(),
                    None,
                    ConnectivityProfile::Clipboard,
                )),
                Arc::new(IrohConnectionAdmission::new(
                    directory.clone(),
                    directory.clone(),
                    Arc::new(Sha256IdentityFingerprintFactory),
                    ledger,
                    ConnectivityProfile::Clipboard,
                )),
                ConnectivityPolicy::default(),
            )
        };
        let clipboard = make_clip(client.clone(), client_ledger.clone());
        let remote_clip = make_clip(server.clone(), server_ledger);
        let (tx, mut rx) = mpsc::channel(8);
        let (clip_tx, mut clip_rx) = mpsc::channel(8);
        let router = Router::builder((*server).clone())
            .accept(
                TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx,
                },
            )
            .accept(
                MULTIPLEXED_CLIPBOARD_ALPN,
                Capture {
                    manager: remote_clip.clone(),
                    events: clip_tx,
                },
            )
            .spawn();
        let stop = CancellationToken::new();
        let lease = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let (remote, raw) = incoming(&mut rx).await;
        assert!(outbound.retain(&lease).await?);
        let clip = clipboard
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await?;
        let (remote_clip_lease, clip_raw) = incoming(&mut clip_rx).await;
        assert_ne!(lease.connection_id(), clip.connection_id());
        assert_eq!(clip.protocol(), ConnectionProtocol::MultiplexedClipboard);
        // 同一已认证对端再发原生连接，进度入站方向名额仍被第一条连接持有。
        let duplicate = client
            .connect(server.addr(), TRANSFER_PROGRESS_ALPN)
            .await?;
        timeout(BUDGET, duplicate.closed()).await?;
        assert!(!raw.is_closed());
        assert!(rx.try_recv().is_err());
        let mut stream = lease.open_bi(Instant::now() + BUDGET).await?;
        stream.send.write_all(&[1]).await?;
        let mut accepted = remote.accept_bi().await?;
        let mut bytes = [0];
        accepted.receive.read_exact(&mut bytes).await?;
        let (io, work) = stream.into_parts();
        let (remote_io, remote_work) = accepted.into_parts();
        drop(io);
        drop(remote_io);
        drop(remote);
        outbound.retire(&lease);
        timeout(BUDGET, raw.closed()).await?;
        let blocked = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &stop)
            .await;
        assert!(blocked.as_ref().err().is_some_and(busy));
        assert!(!clip.is_closed());
        assert!(!clip_raw.is_closed());
        clip.check_current().await?;
        // 其他profile继续真实收发，不因progress退休或其方向名额占用而关闭。
        let mut send = clip.open_bi(Instant::now() + BUDGET).await?;
        send.send.write_all(&[2]).await?;
        let mut receive = remote_clip_lease.accept_bi().await?;
        receive.receive.read_exact(&mut bytes).await?;
        assert_eq!(bytes, [2]);
        drop(send);
        drop(receive);
        drop(lease);
        drop(work);
        drop(remote_work);
        let next = timeout(BUDGET, async {
            loop {
                match outbound
                    .request(peer)
                    .connect(Instant::now() + BUDGET, &stop)
                    .await
                {
                    Ok(lease) => break Ok(lease),
                    Err(source) if busy(&source) => tokio::task::yield_now().await,
                    Err(source) => break Err(source),
                }
            }
        })
        .await??;
        let (remote_next, raw_next) = incoming(&mut rx).await;
        assert!(outbound.retain(&next).await?);
        // 故意把Clipboard backend接到progress准入，必须拒绝实际协议而不是借用其grant。
        let identity_gate = Arc::new(Semaphore::new(0));
        let wrong = ConnectivityManager::new(
            Arc::new(IrohConnectivityBackend::new(
                client.clone(),
                directory.clone(),
                None,
                ConnectivityProfile::Clipboard,
            )),
            Arc::new(IrohConnectionAdmission::new(
                Arc::new(HeldIdentities {
                    directory: directory.clone(),
                    release: identity_gate.clone(),
                }),
                directory.clone(),
                Arc::new(Sha256IdentityFingerprintFactory),
                client_ledger.clone(),
                ConnectivityProfile::TransferProgress,
            )),
            transfer_progress_connectivity_policy(Arc::new(SystemElapsedClock)),
        );
        let attempt = wrong.clone();
        let attempt_stop = stop.clone();
        let rejected = tokio::spawn(async move {
            attempt
                .request(peer)
                .connect(Instant::now() + BUDGET, &attempt_stop)
                .await
        });
        // 先让服务端真实准入再继续客户端身份读取，避免错误协议拒绝与接收回调竞态。
        let (wrong_remote, wrong_raw) = incoming(&mut clip_rx).await;
        identity_gate.add_permits(1);
        assert!(rejected.await?.is_err());
        timeout(BUDGET, wrong_raw.closed()).await?;
        drop(wrong_remote);
        assert!(!clip.is_closed());
        directory.allowed.store(false, Ordering::SeqCst);
        client_ledger.close_device(&peer);
        assert!(next.check_current().await.is_err());
        assert!(clip.check_current().await.is_err());
        timeout(BUDGET, raw_next.closed()).await?;
        timeout(BUDGET, clip_raw.closed()).await?;
        drop(next);
        drop(remote_next);
        drop(clip);
        drop(remote_clip_lease);
        wrong.shutdown().await?;
        outbound.shutdown().await?;
        inbound.shutdown().await?;
        clipboard.shutdown().await?;
        remote_clip.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

struct SuspendClock(AtomicU64);
impl ElapsedClockPort for SuspendClock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elapsed_jump_retires_real_idle_connection_after_actual_work_finishes() -> anyhow::Result<()>
{
    timeout(BUDGET, async {
        let clock = Arc::new(SuspendClock(AtomicU64::new(0)));
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("server");
        let directory = Arc::new(Directory {
            peers: vec![
                (peer, server.addr()),
                (DeviceId::new("client"), client.addr()),
            ],
            allowed: AtomicBool::new(true),
        });
        let outbound = ConnectivityManager::new(
            Arc::new(IrohConnectivityBackend::new(
                client.clone(),
                directory.clone(),
                None,
                ConnectivityProfile::TransferProgress,
            )),
            Arc::new(IrohConnectionAdmission::new(
                directory.clone(),
                directory.clone(),
                Arc::new(Sha256IdentityFingerprintFactory),
                AdmittedConnections::default(),
                ConnectivityProfile::TransferProgress,
            )),
            transfer_progress_connectivity_policy(clock.clone()),
        );
        let inbound = manager(server.clone(), directory, AdmittedConnections::default());
        let (tx, mut rx) = mpsc::channel(8);
        let router = Router::builder((*server).clone())
            .accept(
                TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx,
                },
            )
            .spawn();
        let lease = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &CancellationToken::new())
            .await?;
        let (remote, raw) = incoming(&mut rx).await;
        let mut stream = lease.open_bi(Instant::now() + BUDGET).await?;
        stream.send.write_all(&[7]).await?;
        let mut accepted = remote.accept_bi().await?;
        let mut byte = [0];
        accepted.receive.read_exact(&mut byte).await?;
        assert_eq!(byte, [7]);
        assert!(outbound.retain(&lease).await?);
        let (io, work) = stream.into_parts();
        drop(io);
        drop(accepted);
        drop(lease);
        drop(remote);
        // 真实流和真实工作租约，经过时间跳变为注入值；不声称物理系统睡眠。
        clock.0.store(120, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!raw.is_closed());
        drop(work);
        clock.0.store(149, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!raw.is_closed());
        clock.0.store(150, Ordering::SeqCst);
        timeout(Duration::from_secs(2), raw.closed()).await?;
        outbound.shutdown().await?;
        inbound.shutdown().await?;
        router.shutdown().await?;
        client.close().await;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
