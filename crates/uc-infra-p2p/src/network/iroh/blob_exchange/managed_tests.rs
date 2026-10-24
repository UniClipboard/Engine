//! 使用实际生产 handler 与未改造 Downloader，验证空预热不占已授权连接名额。
mod isolation;
use super::{IrohBlobExchangeAdapter, IrohBlobHandler};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::blobs::{IrohBlobTransferAdapter, BLOBS_ALPN};
use crate::network::iroh::connectivity::{ConnectivityProfile, IrohConnectionAdmission};
use async_trait::async_trait;
use bytes::Bytes;
use iroh::{endpoint::presets, protocol::Router, Endpoint, RelayMode};
use iroh_blobs::store::fs::FsStore;
use iroh_tickets::Ticket;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobShutdownWait, BlobTransport, ConnectivityBackend, ConnectivityError, KnownPeerIdentity,
    MembershipLedgerError, PeerConnection, PeerIdentityDirectoryPort,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::ports::blob::{BlobTransferPort, TagReason};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

struct NoDial;
#[async_trait]
impl ConnectivityBackend for NoDial {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        panic!("provider must not dial");
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
    async fn is_admitted(&self, _: &DeviceId) -> Result<bool, PeerAdmissionError> {
        Ok(self.admitted.load(Ordering::SeqCst))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_provider_serves_legacy_downloader_while_empty_warmup_is_still_open(
) -> anyhow::Result<()> {
    let server = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let client = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let source_dir = tempfile::tempdir()?;
    let destination_dir = tempfile::tempdir()?;
    let source = FsStore::load(source_dir.path()).await?;
    let destination = FsStore::load(destination_dir.path()).await?;
    let members = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: DeviceId::new("legacy-requester"),
            identity_fingerprint: Sha256IdentityFingerprintFactory
                .from_public_key(client.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let admission = Arc::new(IrohConnectionAdmission::new(
        members.clone(),
        members.clone(),
        Arc::new(Sha256IdentityFingerprintFactory),
        AdmittedConnections::default(),
        ConnectivityProfile::Blob,
    ));
    let exchange = Arc::new(IrohBlobExchangeAdapter::new(source.clone()));
    let owner = BlobTransport::new(Arc::new(NoDial), admission, exchange.clone(), exchange);
    let router = Router::builder(server.clone())
        .accept(BLOBS_ALPN, IrohBlobHandler::new(owner.clone()))
        .spawn();
    let source_port = IrohBlobTransferAdapter::new(Arc::new(server.clone()), source.clone());
    let bytes = Bytes::from(vec![57; 512 * 1024]);
    let digest = source_port
        .publish(bytes.clone(), TagReason::ClipboardEntry(EntryId::new()))
        .await?;
    let ticket = source_port.issue_ticket(&digest).await?;
    // 只有历史客户端fixture保留SDK Downloader；实际提供端仍走生产Uni负责人。
    let warmup = client.connect(server.addr(), BLOBS_ALPN).await?;
    let legacy = destination.downloader(&client);
    let native = iroh_blobs::ticket::BlobTicket::decode_bytes(ticket.as_bytes())?;
    timeout(
        Duration::from_secs(10),
        legacy.download(native.hash_and_format(), [native.addr().id]),
    )
    .await??;
    let received = destination.blobs().get_bytes(native.hash()).await?;
    assert_eq!(received, bytes);
    assert!(
        warmup.close_reason().is_none(),
        "data fetch depended on evicting warmup"
    );
    assert!(
        destination
            .blobs()
            .has(iroh_blobs::Hash::from_bytes(*digest.as_bytes()))
            .await?
    );
    owner.shutdown(BlobShutdownWait::Complete).await?;
    router.shutdown().await?;
    client.close().await;
    drop(legacy);
    destination.shutdown().await?;
    Ok(())
}

struct ExistingConnection {
    connection: Arc<dyn PeerConnection>,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl ConnectivityBackend for ExistingConnection {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.connection.clone())
    }
}
struct Progress;
impl uc_application::deps::BlobValidatedProgress for Progress {
    fn validated_through(
        &self,
        _: u64,
        _: u64,
    ) -> Result<(), uc_application::deps::BlobExchangeError> {
        Ok(())
    }
}
struct PausedReceive {
    inner: Box<dyn uc_application::deps::PeerReceiveStream>,
}
impl tokio::io::AsyncRead for PausedReceive {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Pending
    }
}
impl uc_application::deps::PeerReceiveStream for PausedReceive {
    fn id(&self) -> u64 {
        self.inner.id()
    }
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        self.inner.stop(code)
    }
}
fn paused(mut stream: uc_application::deps::StreamLease) -> uc_application::deps::StreamLease {
    stream.receive = Box::new(PausedReceive {
        inner: stream.receive,
    });
    stream
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_provider_preserves_sibling_after_stream_cancel_and_settles_peer_disconnect(
) -> anyhow::Result<()> {
    use crate::network::iroh::connectivity::IrohPeerConnection;
    use iroh::endpoint::{QuicTransportConfig, VarInt};
    use std::sync::atomic::AtomicUsize;
    use tokio::time::{sleep, Instant};
    use uc_application::deps::{
        BlobExchangeFailure, BlobExchangePort, ConnectivityManager, ConnectivityPolicy,
    };
    let server = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let client = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .transport_config(
            QuicTransportConfig::builder()
                .stream_receive_window(VarInt::from_u32(64 * 1024))
                .build(),
        )
        .bind()
        .await?;
    let source_dir = tempfile::tempdir()?;
    let destination_dir = tempfile::tempdir()?;
    let source = FsStore::load(source_dir.path()).await?;
    let destination = FsStore::load(destination_dir.path()).await?;
    let requester = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: DeviceId::new("requester"),
            identity_fingerprint: Sha256IdentityFingerprintFactory
                .from_public_key(client.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let admission = Arc::new(IrohConnectionAdmission::new(
        requester.clone(),
        requester,
        Arc::new(Sha256IdentityFingerprintFactory),
        AdmittedConnections::default(),
        ConnectivityProfile::Blob,
    ));
    let exchange = Arc::new(IrohBlobExchangeAdapter::new(source.clone()));
    let owner = BlobTransport::new(Arc::new(NoDial), admission, exchange.clone(), exchange);
    let router = Router::builder(server.clone())
        .accept(BLOBS_ALPN, IrohBlobHandler::new(owner.clone()))
        .spawn();
    let publisher = IrohBlobTransferAdapter::new(Arc::new(server.clone()), source);
    let first = publisher
        .publish(
            Bytes::from(vec![41; 4 * 1024 * 1024]),
            TagReason::ClipboardEntry(EntryId::new()),
        )
        .await?;
    let bytes = Bytes::from(vec![71; 4 * 1024 * 1024]);
    let second = publisher
        .publish(bytes.clone(), TagReason::ClipboardEntry(EntryId::new()))
        .await?;
    let native = client.connect(server.addr(), BLOBS_ALPN).await?;
    let provider = DeviceId::new("provider");
    let peers = Arc::new(Members {
        identities: vec![KnownPeerIdentity {
            device_id: provider,
            identity_fingerprint: Sha256IdentityFingerprintFactory
                .from_public_key(server.id().as_bytes())?,
        }],
        admitted: AtomicBool::new(true),
    });
    let backend = Arc::new(ExistingConnection {
        connection: IrohPeerConnection::wrap(native.clone()),
        calls: AtomicUsize::new(0),
    });
    let manager = ConnectivityManager::new(
        backend.clone(),
        Arc::new(IrohConnectionAdmission::new(
            peers.clone(),
            peers,
            Arc::new(Sha256IdentityFingerprintFactory),
            AdmittedConnections::default(),
            ConnectivityProfile::Blob,
        )),
        ConnectivityPolicy {
            outbound_per_peer: 2,
            connections_per_peer_direction: Some(1),
            ..ConnectivityPolicy::default()
        },
    );
    let lease = manager
        .request(provider)
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await?;
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let store = destination.clone();
    let stream = paused(
        lease
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?,
    );
    let a = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(store)
            .receive(stream, first, token, Arc::new(Progress))
            .await
    });
    // 使用实际传输统计确认提供端已写出正文，而不是只取消一个尚未开始的请求。
    timeout(Duration::from_secs(5), async {
        while native.stats().udp_rx.bytes < 32 * 1024 {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await?;
    let store = destination.clone();
    let stream = lease
        .open_bi(Instant::now() + Duration::from_secs(5))
        .await?;
    let b = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(store)
            .receive(stream, second, CancellationToken::new(), Arc::new(Progress))
            .await
    });
    cancel.cancel();
    let cancelled = timeout(Duration::from_secs(5), a)
        .await??
        .err()
        .ok_or_else(|| anyhow::anyhow!("cancelled receive succeeded"))?;
    assert_eq!(cancelled.kind, BlobExchangeFailure::Cancelled);
    let received = timeout(Duration::from_secs(5), b).await???;
    assert_eq!(received.size, bytes.len() as u64);
    assert_eq!(
        destination
            .blobs()
            .get_bytes(iroh_blobs::Hash::from_bytes(*second.as_bytes()))
            .await?,
        bytes
    );
    drop(received);
    assert!(native.close_reason().is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);

    let before = native.stats().udp_rx.bytes;
    let disconnect_cancel = CancellationToken::new();
    let token = disconnect_cancel.clone();
    let store = destination.clone();
    let stream = paused(
        lease
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?,
    );
    let disconnected = tokio::spawn(async move {
        IrohBlobExchangeAdapter::new(store)
            .receive(stream, first, token, Arc::new(Progress))
            .await
    });
    timeout(Duration::from_secs(5), async {
        while native.stats().udp_rx.bytes < before + 32 * 1024 {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await?;
    native.close(0u32.into(), b"test peer departed");
    disconnect_cancel.cancel();
    assert!(timeout(Duration::from_secs(5), disconnected)
        .await??
        .is_err());
    owner.shutdown(BlobShutdownWait::Complete).await?;
    manager.shutdown().await?;
    router.shutdown().await?;
    client.close().await;
    destination.shutdown().await?;
    Ok(())
}
