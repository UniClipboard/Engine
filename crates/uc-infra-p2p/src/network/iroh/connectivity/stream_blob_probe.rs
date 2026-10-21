//! 固定 SDK 与真实 Uni 租约的最小交换证明；成员授权由后续身份矩阵另证。
use super::super::{ConnectivityProfile, IrohConnectionAdmission, IrohPeerConnection};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::blob_exchange::IrohBlobExchangeAdapter;
use async_trait::async_trait;
use futures_util::task::AtomicWaker;
use iroh::{Endpoint, RelayMode};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{ticket::BlobTicket, BlobFormat};
use iroh_tickets::Ticket;
use std::error::Error;
use std::io;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::Notify;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangePort, BlobValidatedProgress, ConnectivityBackend,
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, KnownPeerIdentity,
    MembershipLedgerError, PeerConnection, PeerIdentityDirectoryPort, PeerReceiveStream,
    SessionConnectionAdmissionPort, StreamLease,
};
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::ports::blob::BlobDigest;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

const BUDGET: Duration = Duration::from_secs(5);
struct Backend {
    connection: Arc<dyn PeerConnection>,
    calls: AtomicUsize,
}
#[async_trait]
impl ConnectivityBackend for Backend {
    async fn connect(
        &self,
        _: DeviceId,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.connection.clone())
    }
}
struct Members {
    identities: Vec<KnownPeerIdentity>,
    admitted: AtomicBool,
}
#[async_trait]
impl PeerIdentityDirectoryPort for Members {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(self.identities.clone())
    }
}
#[async_trait]
impl PeerAdmissionPort for Members {
    async fn is_admitted(&self, peer: &DeviceId) -> Result<bool, PeerAdmissionError> {
        Ok(self.admitted.load(Ordering::SeqCst)
            && self.identities.iter().any(|row| row.device_id == *peer))
    }
}
struct Progress;
impl BlobValidatedProgress for Progress {
    fn validated_through(&self, _: u64, _: u64) -> Result<(), BlobExchangeError> {
        Ok(())
    }
}
#[derive(Default)]
struct Pause {
    entered: Notify,
    resume: AtomicBool,
    waker: AtomicWaker,
}
impl Pause {
    fn release(&self) {
        self.resume.store(true, Ordering::SeqCst);
        self.waker.wake();
    }
}
struct Receive {
    stream: Box<dyn PeerReceiveStream>,
    read: usize,
    limit: Option<usize>,
    paused: Arc<Pause>,
}
impl AsyncRead for Receive {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.limit.is_some_and(|limit| self.read >= limit)
            && !self.paused.resume.load(Ordering::SeqCst)
        {
            self.paused.waker.register(cx.waker());
            if !self.paused.resume.load(Ordering::SeqCst) {
                self.paused.entered.notify_one();
                return Poll::Pending;
            }
        }
        let before = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        self.read += buf.filled().len() - before;
        result
    }
}
impl PeerReceiveStream for Receive {
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        self.stream.stop(code)
    }
    fn id(&self) -> u64 {
        self.stream.id()
    }
}
fn paused(mut stream: StreamLease, pause: Arc<Pause>) -> StreamLease {
    stream.receive = Box::new(Receive {
        stream: stream.receive,
        read: 0,
        limit: Some(64 * 1024),
        paused: pause,
    });
    stream
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uni_blob_stream_cancel_preserves_sibling_and_local_work_capacity() -> anyhow::Result<()> {
    prove_isolation(false).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uni_same_content_cancel_preserves_independent_waiter() -> anyhow::Result<()> {
    prove_isolation(true).await
}
async fn prove_isolation(same_content: bool) -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source = FsStore::load(temp.path().join("source")).await?;
    let destination = FsStore::load(temp.path().join("destination")).await?;
    let server = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await?;
    let client = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    timeout(BUDGET, async {
        while server.addr().ip_addrs().next().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let (native_client, native_server) =
        tokio::join!(client.connect(server.addr(), iroh_blobs::ALPN), async {
            server.accept().await.expect("accepted connection").await
        },);
    let native_client = native_client?;
    let native_server = native_server?;
    assert_eq!(native_client.alpn(), iroh_blobs::ALPN);
    let peer = DeviceId::new("blob-provider");
    let client_backend = Arc::new(Backend {
        connection: IrohPeerConnection::wrap(native_client.clone()),
        calls: AtomicUsize::new(0),
    });
    let server_backend = Arc::new(Backend {
        connection: IrohPeerConnection::wrap(native_server.clone()),
        calls: AtomicUsize::new(0),
    });
    let receiver_peer = DeviceId::new("blob-receiver");
    let fingerprint = Arc::new(Sha256IdentityFingerprintFactory);
    let client_members = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: peer,
            identity_fingerprint: fingerprint.from_public_key(server.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let server_members = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: receiver_peer,
            identity_fingerprint: fingerprint.from_public_key(client.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let server_registry = AdmittedConnections::default();
    let client_admission = Arc::new(IrohConnectionAdmission::new(
        client_members.clone(),
        client_members,
        fingerprint.clone(),
        AdmittedConnections::default(),
        ConnectivityProfile::Blob,
    ));
    let server_admission = Arc::new(IrohConnectionAdmission::new(
        server_members.clone(),
        server_members.clone(),
        fingerprint,
        server_registry.clone(),
        ConnectivityProfile::Blob,
    ));
    let downloading = ConnectivityManager::new(
        client_backend.clone(),
        client_admission,
        ConnectivityPolicy {
            outbound_per_peer: 2,
            ..ConnectivityPolicy::default()
        },
    );
    let providing = ConnectivityManager::new(
        server_backend.clone(),
        server_admission.clone(),
        ConnectivityPolicy {
            inbound_per_connection: 2,
            ..ConnectivityPolicy::default()
        },
    );
    let local = downloading.connect(peer, Instant::now() + BUDGET).await?;
    let remote = providing.accept(server_backend.connection.clone()).await?;
    let tag = source.blobs().add_bytes(vec![41; 4 * 1024 * 1024]).await?;
    let mut stream_a = local.open_bi(Instant::now() + BUDGET).await?;
    assert_eq!(stream_a.send.id(), stream_a.receive.id());
    for error in [
        stream_a.send.reset(u64::MAX).unwrap_err(),
        stream_a.receive.stop(u64::MAX).unwrap_err(),
    ] {
        assert_eq!(error.class(), "connection_stream_failed");
        assert!(error.source().is_some());
    }
    assert!(native_client.close_reason().is_none());
    let pause_a = Arc::new(Pause::default());
    let paused = pause_a.clone();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let saved = destination.clone();
    let get_a = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(saved)
            .receive(
                self::paused(stream_a, paused),
                BlobDigest::from_bytes(*tag.hash.as_bytes()),
                stop,
                Arc::new(Progress),
            )
            .await
    });
    let stream_a = remote.accept_bi().await?;
    let providing_a = source.clone();
    let serve_a = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(providing_a)
            .provide(stream_a, Instant::now() + BUDGET, CancellationToken::new())
            .await
    });
    timeout(BUDGET, pause_a.entered.notified()).await?;
    let bytes = vec![if same_content { 41 } else { 71 }; 4 * 1024 * 1024];
    let sibling = source.blobs().add_bytes(bytes.clone()).await?;
    let refreshed = downloading.connect(peer, Instant::now() + BUDGET).await?;
    assert_eq!(refreshed.connection_id(), local.connection_id());
    assert_eq!(client_backend.calls.load(Ordering::SeqCst), 1);
    let stream_b = refreshed.open_bi(Instant::now() + BUDGET).await?;
    let saved = destination.clone();
    let pause_b = Arc::new(Pause::default());
    let paused = pause_b.clone();
    let get_b = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(saved)
            .receive(
                self::paused(stream_b, paused),
                BlobDigest::from_bytes(*sibling.hash.as_bytes()),
                CancellationToken::new(),
                Arc::new(Progress),
            )
            .await
    });
    let stream_b = remote.accept_bi().await?;
    let providing_b = source.clone();
    let serve_b = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(providing_b)
            .provide(stream_b, Instant::now() + BUDGET, CancellationToken::new())
            .await
    });
    if !same_content {
        timeout(BUDGET, pause_b.entered.notified()).await?;
    }
    assert!(!get_b.is_finished());
    cancel.cancel();
    assert!(timeout(BUDGET, get_a).await??.is_err());
    let _ = timeout(BUDGET, serve_a).await??;
    assert!(!get_b.is_finished());
    assert!(native_client.close_reason().is_none());
    if same_content {
        timeout(BUDGET, pause_b.entered.notified()).await?;
    }
    pause_b.release();
    timeout(BUDGET, serve_b).await???;
    let completed = timeout(BUDGET, get_b).await???;
    assert_eq!(completed.size, bytes.len() as u64);
    assert_eq!(
        destination.blobs().get_bytes(sibling.hash).await?.as_ref(),
        bytes
    );
    drop(completed);
    assert!(native_client.close_reason().is_none());
    // ticket 仍合法且来源内容仍在；当前成员撤销与会话代次决定授权。
    let ticket = BlobTicket::new(server.addr(), sibling.hash, BlobFormat::Raw).encode_bytes();
    let old_attempt = server_admission.begin(receiver_peer);
    server_members.admitted.store(false, Ordering::SeqCst);
    assert!(matches!(
        remote.check_current().await,
        Err(ConnectivityError::Denied)
    ));
    let fresh = server_admission.begin(receiver_peer);
    assert!(matches!(
        fresh.adopt(server_backend.connection.clone()).await,
        Err(ConnectivityError::Denied)
    ));
    server_registry.close_device(&receiver_peer, b"test member revoked");
    server_members.admitted.store(true, Ordering::SeqCst);
    assert!(matches!(
        old_attempt.check_current().await,
        Err(ConnectivityError::Denied)
    ));
    assert!(matches!(
        remote.check_current().await,
        Err(ConnectivityError::Denied)
    ));
    let session_attempt = server_admission.begin(receiver_peer);
    server_registry.close_all(b"test session retired");
    assert!(matches!(
        session_attempt.check_current().await,
        Err(ConnectivityError::Denied)
    ));
    assert_eq!(BlobTicket::decode_bytes(&ticket)?.hash(), sibling.hash);
    assert!(source.blobs().has(sibling.hash).await?);
    downloading.shutdown().await?;
    providing.shutdown().await?;
    client.close().await;
    server.close().await;
    source.shutdown().await?;
    destination.shutdown().await?;
    Ok(())
}
