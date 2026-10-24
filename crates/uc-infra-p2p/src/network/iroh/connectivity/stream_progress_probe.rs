//! 旧进度字节经过真实 Uni 单向流，停止只影响本流，实际工作持有原许可。
use super::super::{ConnectivityProfile, IrohConnectionAdmission, IrohPeerConnection};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::transfer_progress_adapter::TRANSFER_PROGRESS_ALPN;
use async_trait::async_trait;
use iroh::{Endpoint, RelayMode};
use iroh_blobs::ALPN as BLOBS_ALPN;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ConnectivityBackend, ConnectivityError, ConnectivityManager,
    ConnectivityPolicy, ElapsedClockPort, KnownPeerIdentity, MembershipLedgerError, PeerConnection,
    PeerIdentityDirectoryPort, ProgressExchangeDeadline,
};
use uc_core::file_transfer::OutboundProgressStatus;
use uc_core::ids::DeviceId;
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::{read_frame, write_frame, ProgressFrame};

const BUDGET: Duration = Duration::from_secs(5);

pub(super) fn progress_budget() -> ProgressExchangeDeadline {
    ProgressExchangeDeadline::new(
        Arc::new(SystemElapsedClock),
        SystemElapsedClock
            .elapsed()
            .expect("native clock")
            .saturating_add(BUDGET),
    )
    .expect("native progress budget")
}

struct Backend(Arc<dyn PeerConnection>);
#[async_trait]
impl ConnectivityBackend for Backend {
    async fn connect(
        &self,
        _: DeviceId,
        _: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        Ok(self.0.clone())
    }
}

struct Members {
    identity: KnownPeerIdentity,
    admitted: AtomicBool,
    read_gate: ReadGate,
}

struct ReadGate {
    armed: AtomicBool,
    entered: Notify,
    released: Semaphore,
}
#[async_trait]
impl PeerIdentityDirectoryPort for Members {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(vec![self.identity.clone()])
    }
}
#[async_trait]
impl PeerAdmissionPort for Members {
    async fn is_admitted(&self, peer: &DeviceId) -> Result<bool, PeerAdmissionError> {
        if self.read_gate.armed.swap(false, Ordering::SeqCst) {
            self.read_gate.entered.notify_one();
            self.read_gate.released.acquire().await.unwrap().forget();
        }
        Ok(self.admitted.load(Ordering::SeqCst) && self.identity.device_id == *peer)
    }
}

pub(super) struct Fixture {
    pub(super) client: Endpoint,
    pub(super) server: Endpoint,
    pub(super) outbound: ConnectivityManager,
    inbound: ConnectivityManager,
    pub(super) sending: ConnectionLease,
    pub(super) receiving: ConnectionLease,
    membership: Arc<Members>,
}

impl Fixture {
    pub(super) async fn new(
        outbound_capacity: usize,
        inbound_capacity: usize,
        alpn: &[u8],
    ) -> anyhow::Result<Self> {
        Self::new_as(
            DeviceId::new("progress-source"),
            outbound_capacity,
            inbound_capacity,
            alpn,
        )
        .await
    }
    pub(super) async fn new_as(
        peer: DeviceId,
        outbound_capacity: usize,
        inbound_capacity: usize,
        alpn: &[u8],
    ) -> anyhow::Result<Self> {
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![alpn.to_vec(), BLOBS_ALPN.to_vec()])
            .bind()
            .await?;
        let client = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        Self::connect_as(
            client,
            server,
            outbound_capacity,
            inbound_capacity,
            alpn,
            peer,
        )
        .await
    }

    pub(super) async fn connect(
        client: Endpoint,
        server: Endpoint,
        outbound_capacity: usize,
        inbound_capacity: usize,
        alpn: &[u8],
    ) -> anyhow::Result<Self> {
        Self::connect_as(
            client,
            server,
            outbound_capacity,
            inbound_capacity,
            alpn,
            DeviceId::new("progress-source"),
        )
        .await
    }
    pub(super) async fn connect_as(
        client: Endpoint,
        server: Endpoint,
        outbound_capacity: usize,
        inbound_capacity: usize,
        alpn: &[u8],
        peer: DeviceId,
    ) -> anyhow::Result<Self> {
        timeout(BUDGET, async {
            while server.addr().ip_addrs().next().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let (sending, receiving) = timeout(BUDGET, async {
            tokio::join!(client.connect(server.addr(), alpn), async {
                server.accept().await.expect("accepted connection").await
            })
        })
        .await?;
        let sending = IrohPeerConnection::wrap(sending?);
        let receiving = IrohPeerConnection::wrap(receiving?);
        let fingerprint = Arc::new(Sha256IdentityFingerprintFactory);
        let membership = Arc::new(Members {
            identity: KnownPeerIdentity {
                device_id: peer,
                identity_fingerprint: fingerprint.from_public_key(server.id().as_bytes())?,
            },
            admitted: AtomicBool::new(true),
            read_gate: ReadGate {
                armed: AtomicBool::new(false),
                entered: Notify::new(),
                released: Semaphore::new(0),
            },
        });
        let incoming_members = Arc::new(Members {
            identity: KnownPeerIdentity {
                device_id: DeviceId::new("progress-receiver"),
                identity_fingerprint: fingerprint.from_public_key(client.id().as_bytes())?,
            },
            admitted: AtomicBool::new(true),
            read_gate: ReadGate {
                armed: AtomicBool::new(false),
                entered: Notify::new(),
                released: Semaphore::new(0),
            },
        });
        let policy = ConnectivityPolicy {
            outbound_per_peer: outbound_capacity,
            inbound_per_connection: inbound_capacity,
            inbound_per_session: inbound_capacity,
            supervise_local_work: true,
            ..ConnectivityPolicy::default()
        };
        // 阶段 0 复用真实成员准入；Router 的进度 profile 接线另验。
        let profile = if alpn == BLOBS_ALPN {
            ConnectivityProfile::Blob
        } else {
            ConnectivityProfile::Clipboard
        };
        let outbound = ConnectivityManager::new(
            Arc::new(Backend(sending.clone())),
            Arc::new(IrohConnectionAdmission::new(
                membership.clone(),
                membership.clone(),
                fingerprint.clone(),
                AdmittedConnections::default(),
                profile,
            )),
            policy,
        );
        let inbound = ConnectivityManager::new(
            Arc::new(Backend(receiving.clone())),
            Arc::new(IrohConnectionAdmission::new(
                incoming_members.clone(),
                incoming_members,
                fingerprint,
                AdmittedConnections::default(),
                profile,
            )),
            policy,
        );
        let sending = outbound
            .request(peer)
            .connect(Instant::now() + BUDGET, &CancellationToken::new())
            .await?;
        let receiving = inbound.accept(receiving).await?;
        Ok(Self {
            client,
            server,
            outbound,
            inbound,
            sending,
            receiving,
            membership,
        })
    }

    pub(super) async fn close(self) -> anyhow::Result<()> {
        drop(self.sending);
        drop(self.receiving);
        self.outbound.shutdown().await?;
        self.inbound.shutdown().await?;
        self.client.close().await;
        self.server.close().await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_progress_uni_stop_preserves_sibling_and_work_capacity() -> anyhow::Result<()> {
    let fixture = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    let cancel = CancellationToken::new();
    let mut first = fixture
        .sending
        .open_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    first.stream.write_all(b"first").await?;
    let incoming_first = fixture
        .receiving
        .accept_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    assert_eq!(first.stream.id(), incoming_first.stream.id());
    let mut sibling = fixture
        .sending
        .open_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    let frame = ProgressFrame {
        transfer_id_bytes: [7; 16],
        bytes_transferred: 1024,
        total_bytes: Some(4096),
        status: OutboundProgressStatus::InProgress,
    };
    write_frame(&mut sibling.stream, &frame).await?;
    sibling.stream.finish()?;
    let mut incoming_sibling = fixture
        .receiving
        .accept_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    let (mut stopped, local_work) = first.into_parts();
    stopped.reset(0)?;
    drop(stopped);
    incoming_first.stop()?;
    let actual = timeout(BUDGET, read_frame(&mut incoming_sibling.stream)).await??;
    assert_eq!(actual, frame);
    assert!(!fixture.sending.is_closed());
    assert!(!fixture.receiving.is_closed());
    // sibling 持一份许可，第一项实际工作持另一份，停止网络不能提前归还。
    assert!(matches!(
        fixture
            .sending
            .open_uni(Instant::now() + Duration::from_millis(40), &cancel)
            .await,
        Err(ConnectivityError::TimedOut(_))
    ));
    drop(local_work);
    let next = fixture
        .sending
        .open_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    next.stop()?;
    drop(sibling);
    drop(incoming_sibling);
    fixture.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_progress_uni_rechecks_current_membership_before_open() -> anyhow::Result<()> {
    let fixture = Fixture::new(1, 1, TRANSFER_PROGRESS_ALPN).await?;
    fixture.membership.admitted.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .sending
            .open_uni(Instant::now() + BUDGET, &CancellationToken::new())
            .await,
        Err(ConnectivityError::Denied)
    ));
    fixture.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_progress_uni_cancelled_waiter_opens_no_stream() -> anyhow::Result<()> {
    let fixture = Fixture::new(1, 1, TRANSFER_PROGRESS_ALPN).await?;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        fixture
            .sending
            .open_uni(Instant::now() + BUDGET, &cancel)
            .await,
        Err(ConnectivityError::Stopped)
    ));
    assert!(timeout(
        Duration::from_millis(40),
        fixture
            .receiving
            .accept_uni(Instant::now() + BUDGET, &CancellationToken::new())
    )
    .await
    .is_err());
    fixture.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_progress_uni_cancel_waits_for_actual_admission_read() -> anyhow::Result<()> {
    let fixture = Fixture::new(1, 1, TRANSFER_PROGRESS_ALPN).await?;
    fixture
        .membership
        .read_gate
        .armed
        .store(true, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let stopping = cancel.clone();
    let sending = fixture.sending.clone();
    let opening =
        tokio::spawn(async move { sending.open_uni(Instant::now() + BUDGET, &stopping).await });
    timeout(BUDGET, fixture.membership.read_gate.entered.notified()).await?;
    cancel.cancel();
    tokio::task::yield_now().await;
    assert!(!opening.is_finished(), "准入读取仍在执行，不能提前交还许可");
    assert!(matches!(
        fixture
            .sending
            .open_uni(
                Instant::now() + Duration::from_millis(40),
                &CancellationToken::new()
            )
            .await,
        Err(ConnectivityError::TimedOut(_))
    ));
    fixture.membership.read_gate.released.add_permits(1);
    assert!(matches!(
        timeout(BUDGET, opening).await??,
        Err(ConnectivityError::Stopped)
    ));
    assert!(timeout(
        Duration::from_millis(40),
        fixture
            .receiving
            .accept_uni(Instant::now() + BUDGET, &CancellationToken::new())
    )
    .await
    .is_err());
    fixture.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_progress_uni_excess_inbound_stops_only_excess_stream() -> anyhow::Result<()> {
    let fixture = Fixture::new(2, 1, TRANSFER_PROGRESS_ALPN).await?;
    let cancel = CancellationToken::new();
    let mut first = fixture
        .sending
        .open_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    first.stream.write_all(b"before").await?;
    let mut incoming_first = fixture
        .receiving
        .accept_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    let mut excess = fixture
        .sending
        .open_uni(Instant::now() + BUDGET, &cancel)
        .await?;
    excess.stream.write_all(b"excess").await?;
    assert!(matches!(
        fixture
            .receiving
            .accept_uni(Instant::now() + BUDGET, &cancel)
            .await,
        Err(ConnectivityError::Busy(_))
    ));
    first.stream.write_all(b"after").await?;
    first.stream.finish()?;
    let mut actual = Vec::new();
    timeout(BUDGET, incoming_first.stream.read_to_end(&mut actual)).await??;
    assert_eq!(actual, b"beforeafter");
    assert!(!fixture.sending.is_closed());
    assert!(!fixture.receiving.is_closed());
    drop(first);
    drop(excess);
    drop(incoming_first);
    fixture.close().await
}
