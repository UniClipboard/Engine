//! 固定 SDK 与真实 Uni 租约的最小交换证明；成员授权由后续身份矩阵另证。
#[path = "../../../../tests/support/blob_exchange.rs"]
mod exchange;

use super::super::IrohPeerConnection;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::task::AtomicWaker;
use iroh::endpoint::VarInt;
use iroh::{Endpoint, RelayMode};
use iroh_blobs::util::{AsyncReadRecvStream, AsyncReadRecvStreamExtra, SendStream};
use iroh_blobs::{
    get::StreamPair as GetPair, provider::StreamPair as ProvidePair, store::fs::FsStore,
};
use std::error::Error;
use std::io;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use tokio::sync::Notify;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ConnectivityBackend, ConnectivityError, ConnectivityManager,
    ConnectivityPolicy, PeerConnection, PeerReceiveStream, PeerSendStream,
    SessionConnectionAdmissionPort, StreamLease, StreamWorkLease,
};
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;

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
#[derive(Clone)]
struct Identity {
    peer: DeviceId,
    key: [u8; 32],
}
struct Grant(DeviceId);
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.0
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
}
#[async_trait]
impl AdmissionAttempt for Identity {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        if connection.authenticated_key() != self.key {
            return Err(ConnectivityError::Denied);
        }
        Ok(Box::new(Grant(self.peer)))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for Identity {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(self.clone())
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        if connection.authenticated_key() != self.key {
            return Err(ConnectivityError::Denied);
        }
        Ok(Box::new(Grant(self.peer)))
    }
}
struct SdkSend {
    stream: Box<dyn PeerSendStream>,
    finished: bool,
}
impl SendStream for SdkSend {
    async fn send_bytes(&mut self, bytes: Bytes) -> io::Result<()> {
        self.stream.write_all(&bytes).await
    }
    async fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes).await
    }
    async fn sync(&mut self) -> io::Result<()> {
        self.stream.flush().await?;
        self.stream.finish().map_err(io::Error::other)?;
        self.finished = true;
        Ok(())
    }
    fn reset(&mut self, code: VarInt) -> io::Result<()> {
        self.finished = true;
        self.stream.reset(u64::from(code)).map_err(io::Error::other)
    }
    async fn stopped(&mut self) -> io::Result<Option<VarInt>> {
        self.stream.settled().await.map_err(io::Error::other)?;
        Ok(None)
    }
    fn id(&self) -> u64 {
        self.stream.id()
    }
}
impl Drop for SdkSend {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.stream.reset(0);
        }
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
impl AsyncReadRecvStreamExtra for Receive {
    fn inner(&mut self) -> &mut (impl AsyncRead + Unpin + Send) {
        self
    }
    fn stop(&mut self, code: VarInt) -> io::Result<()> {
        self.stream.stop(u64::from(code)).map_err(io::Error::other)
    }
    fn id(&self) -> u64 {
        self.stream.id()
    }
}
impl Drop for Receive {
    fn drop(&mut self) {
        let _ = self.stream.stop(0);
    }
}
fn split(
    stream: StreamLease,
    pause: Option<Arc<Pause>>,
) -> (SdkSend, AsyncReadRecvStream<Receive>, StreamWorkLease) {
    let ((send, receive), work) = stream.into_parts();
    let limit = pause.as_ref().map(|_| 64 * 1024);
    (
        SdkSend {
            stream: send,
            finished: false,
        },
        AsyncReadRecvStream::new(Receive {
            stream: receive,
            read: 0,
            limit,
            paused: pause.unwrap_or_else(|| Arc::new(Pause::default())),
        }),
        work,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uni_blob_stream_cancel_preserves_sibling_and_local_work_capacity() -> anyhow::Result<()> {
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
    let downloading = ConnectivityManager::new(
        client_backend.clone(),
        Arc::new(Identity {
            peer,
            key: *server.id().as_bytes(),
        }),
        ConnectivityPolicy {
            outbound_per_peer: 2,
            ..ConnectivityPolicy::default()
        },
    );
    let providing = ConnectivityManager::new(
        server_backend.clone(),
        Arc::new(Identity {
            peer: DeviceId::new("blob-receiver"),
            key: *client.id().as_bytes(),
        }),
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
        let (send, receive, _work) = split(stream_a, Some(paused));
        exchange::get(
            saved,
            GetPair::new(0, receive, send),
            tag.hash,
            stop,
            Arc::new(Notify::new()),
        )
        .await
    });
    let stream_a = remote.accept_bi().await?;
    let providing_a = source.clone();
    let serve_a = tokio::spawn(async move {
        let (send, receive, _work) = split(stream_a, None);
        exchange::provide(
            providing_a,
            ProvidePair::new(0, receive, send, Default::default()),
            CancellationToken::new(),
        )
        .await
    });
    timeout(BUDGET, pause_a.entered.notified()).await?;
    let bytes = vec![71; 4 * 1024 * 1024];
    let sibling = source.blobs().add_bytes(bytes.clone()).await?;
    let refreshed = downloading.connect(peer, Instant::now() + BUDGET).await?;
    assert_eq!(refreshed.connection_id(), local.connection_id());
    assert_eq!(client_backend.calls.load(Ordering::SeqCst), 1);
    let stream_b = refreshed.open_bi(Instant::now() + BUDGET).await?;
    let saved = destination.clone();
    let pause_b = Arc::new(Pause::default());
    let paused = pause_b.clone();
    let get_b = tokio::spawn(async move {
        let (send, receive, _work) = split(stream_b, Some(paused));
        exchange::get(
            saved,
            GetPair::new(0, receive, send),
            sibling.hash,
            CancellationToken::new(),
            Arc::new(Notify::new()),
        )
        .await
    });
    let stream_b = remote.accept_bi().await?;
    let providing_b = source.clone();
    let serve_b = tokio::spawn(async move {
        let (send, receive, _work) = split(stream_b, None);
        exchange::provide(
            providing_b,
            ProvidePair::new(0, receive, send, Default::default()),
            CancellationToken::new(),
        )
        .await
    });
    timeout(BUDGET, pause_b.entered.notified()).await?;
    assert!(!get_b.is_finished());
    cancel.cancel();
    assert!(timeout(BUDGET, get_a).await??.is_err());
    let _ = timeout(BUDGET, serve_a).await??;
    assert!(!get_b.is_finished());
    assert!(native_client.close_reason().is_none());
    pause_b.release();
    timeout(BUDGET, serve_b).await???;
    timeout(BUDGET, get_b).await???;
    assert_eq!(
        destination.blobs().get_bytes(sibling.hash).await?.as_ref(),
        bytes
    );
    assert!(native_client.close_reason().is_none());
    downloading.shutdown().await?;
    providing.shutdown().await?;
    client.close().await;
    server.close().await;
    source.shutdown().await?;
    destination.shutdown().await?;
    Ok(())
}
