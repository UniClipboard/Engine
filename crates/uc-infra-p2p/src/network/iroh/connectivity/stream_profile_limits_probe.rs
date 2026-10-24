use super::*;
use std::sync::atomic::AtomicU64;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use uc_application::deps::ConnectivityPolicy;
use uc_application::deps::{ElapsedClockError, ElapsedClockPort};
use uc_core::file_transfer::OutboundProgressStatus;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::clipboard::MULTIPLEXED_CLIPBOARD_ALPN;
use uc_sync_protocol::transfer_progress::{self, ProgressFrame, LEGACY_TRANSFER_PROGRESS_ALPN};

async fn incoming(rx: &mut mpsc::Receiver<Incoming>) -> Incoming {
    timeout(BUDGET, rx.recv()).await.unwrap().unwrap()
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
        client_ledger.close_device(&peer, b"test_revoked");
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
