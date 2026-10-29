//! 第二backend只提供内存字节流及测试固定身份，不实现进度协议或业务规则。
use async_trait::async_trait;
use futures_util::future::BoxFuture;
use futures_util::task::noop_waker_ref;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{duplex, AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::{mpsc, watch, Mutex};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    transfer_progress_connectivity_policy, AdmissionAttempt, AdmissionGrant, ConnectionLease,
    ConnectionMode, ConnectionProtocol, ConnectivityBackend, ConnectivityError,
    ConnectivityManager, InboundAdmissionFence, InboundIdentityReservation, PeerConnection,
    PeerReceiveStream, PeerSendStream, SessionConnectionAdmissionPort, StreamPair,
};
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;
use uc_infra_local::SystemElapsedClock;

struct MemorySendStream {
    id: u64,
    stream: Option<DuplexStream>,
}
impl AsyncWrite for MemorySendStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.stream.as_mut() {
            Some(stream) => Pin::new(stream).poll_write(cx, bytes),
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.stream.as_mut() {
            Some(stream) => Pin::new(stream).poll_flush(cx),
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.stream.as_mut() {
            Some(stream) => Pin::new(stream).poll_shutdown(cx),
            None => Poll::Ready(Ok(())),
        }
    }
}
impl PeerSendStream for MemorySendStream {
    fn id(&self) -> u64 {
        self.id
    }
    fn reset(&mut self, _: u64) -> Result<(), ConnectivityError> {
        self.stream.take();
        Ok(())
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        // DuplexStream的shutdown同步设置EOF；不能把未结束的写入伪装成FIN。
        let mut cx = Context::from_waker(noop_waker_ref());
        match Pin::new(self).poll_shutdown(&mut cx) {
            Poll::Ready(result) => {
                result.map_err(|source| ConnectivityError::Stream(Box::new(source)))
            }
            Poll::Pending => panic!("memory byte stream shutdown must finish synchronously"),
        }
    }
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async { Ok(()) })
    }
}
struct MemoryReceiveStream {
    id: u64,
    stream: Option<DuplexStream>,
}
impl AsyncRead for MemoryReceiveStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.stream.as_mut() {
            Some(stream) => Pin::new(stream).poll_read(cx, bytes),
            None => Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())),
        }
    }
}
impl PeerReceiveStream for MemoryReceiveStream {
    fn id(&self) -> u64 {
        self.id
    }
    fn stop(&mut self, _: u64) -> Result<(), ConnectivityError> {
        self.stream.take();
        Ok(())
    }
}
struct MemoryConnection {
    id: usize,
    mode: ConnectionMode,
    protocol: ConnectionProtocol,
    remote_key: [u8; 32],
    closed: watch::Sender<bool>,
    streams: Arc<AtomicU64>,
    outgoing: mpsc::Sender<StreamPair>,
    incoming: Mutex<mpsc::Receiver<StreamPair>>,
}
#[async_trait]
impl PeerConnection for MemoryConnection {
    fn id(&self) -> usize {
        self.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        self.remote_key
    }
    fn mode(&self) -> ConnectionMode {
        self.mode
    }
    fn protocol(&self) -> ConnectionProtocol {
        self.protocol
    }
    fn channel(&self) -> ConnectionChannel {
        ConnectionChannel::Unknown
    }
    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }
    fn close(&self) {
        self.closed.send_replace(true);
    }
    async fn closed(&self) {
        let mut closed = self.closed.subscribe();
        while !*closed.borrow_and_update() {
            closed.changed().await.unwrap();
        }
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        if self.is_closed() {
            return Err(ConnectivityError::Stopped);
        }
        let id = self.streams.fetch_add(1, Ordering::SeqCst);
        let (sending, remote_receive) = duplex(65536);
        let (remote_send, receiving) = duplex(65536);
        self.outgoing
            .send((
                Box::new(MemorySendStream {
                    id,
                    stream: Some(remote_send),
                }),
                Box::new(MemoryReceiveStream {
                    id,
                    stream: Some(remote_receive),
                }),
            ))
            .await
            .unwrap();
        Ok((
            Box::new(MemorySendStream {
                id,
                stream: Some(sending),
            }),
            Box::new(MemoryReceiveStream {
                id,
                stream: Some(receiving),
            }),
        ))
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.incoming
            .lock()
            .await
            .recv()
            .await
            .ok_or(ConnectivityError::Stopped)
    }
}
fn pair(
    id: usize,
    left_remote: [u8; 32],
    right_remote: [u8; 32],
) -> (Arc<MemoryConnection>, Arc<MemoryConnection>) {
    pair_with_protocol(
        id,
        left_remote,
        right_remote,
        ConnectionMode::Multiplexed,
        ConnectionProtocol::TransferProgressV2,
    )
}
fn pair_with_protocol(
    id: usize,
    left_remote: [u8; 32],
    right_remote: [u8; 32],
    mode: ConnectionMode,
    protocol: ConnectionProtocol,
) -> (Arc<MemoryConnection>, Arc<MemoryConnection>) {
    let (left, left_incoming) = mpsc::channel(16);
    let (right, right_incoming) = mpsc::channel(16);
    let (closed, _) = watch::channel(false);
    let streams = Arc::new(AtomicU64::new(0));
    (
        Arc::new(MemoryConnection {
            id,
            mode,
            protocol,
            remote_key: left_remote,
            closed: closed.clone(),
            streams: streams.clone(),
            outgoing: right,
            incoming: Mutex::new(left_incoming),
        }),
        Arc::new(MemoryConnection {
            id,
            mode,
            protocol,
            remote_key: right_remote,
            closed,
            streams,
            outgoing: left,
            incoming: Mutex::new(right_incoming),
        }),
    )
}

pub(in crate::network::iroh::connectivity) fn recovery_connection_pair(
    id: usize,
    remote_key: [u8; 32],
) -> (Arc<dyn PeerConnection>, Arc<dyn PeerConnection>) {
    let (left, right) = pair_with_protocol(
        id,
        remote_key,
        [1; 32],
        ConnectionMode::SingleExchange,
        ConnectionProtocol::MembershipBranchRecovery,
    );
    (left, right)
}
struct MemoryBackend {
    target: DeviceId,
    connection: Arc<MemoryConnection>,
    dials: Arc<AtomicUsize>,
}
#[async_trait]
impl ConnectivityBackend for MemoryBackend {
    async fn connect(
        &self,
        target: DeviceId,
        _: Option<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if target != self.target {
            return Err(ConnectivityError::AddressMissing);
        }
        self.dials.fetch_add(1, Ordering::SeqCst);
        Ok(self.connection.clone())
    }
}
#[derive(Clone)]
pub(in crate::network::iroh::connectivity) struct Members {
    pub peer: DeviceId,
    pub key: [u8; 32],
    pub allowed: Arc<AtomicBool>,
}
impl InboundAdmissionFence for Members {
    fn check_current(&self, peer: &DeviceId) -> Result<(), ConnectivityError> {
        if peer == &self.peer && self.allowed.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
struct Grant(Members);
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.0.peer
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.0.check_current(&self.0.peer)
    }
    async fn can_retain(&self) -> Result<bool, ConnectivityError> {
        self.check_current().await?;
        Ok(true)
    }
}
struct Attempt {
    target: DeviceId,
    members: Members,
}
#[async_trait]
impl AdmissionAttempt for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.members.check_current(&self.target)
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.check_current().await?;
        if connection.authenticated_key() != self.members.key {
            return Err(ConnectivityError::Denied);
        }
        Ok(Box::new(Grant(self.members.clone())))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for Members {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        Some(Box::new(self.clone()))
    }
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            target,
            members: self.clone(),
        })
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        if connection.authenticated_key() != self.key {
            return Err(ConnectivityError::Denied);
        }
        reservation.identified(self.peer)?;
        self.check_current(&self.peer)?;
        Ok(Box::new(Grant(self.clone())))
    }
}
pub(super) struct MemoryPair {
    pub sender_id: DeviceId,
    pub receiver_id: DeviceId,
    pub sender: ConnectivityManager,
    pub receiver: ConnectivityManager,
    pub at_sender: ConnectionLease,
    pub at_receiver: ConnectionLease,
    pub allowed: Arc<AtomicBool>,
    pub sender_dials: Arc<AtomicUsize>,
    pub receiver_dials: Arc<AtomicUsize>,
}
impl MemoryPair {
    pub async fn new() -> anyhow::Result<Self> {
        let sender_id = DeviceId::new("memory-original-sender");
        let receiver_id = DeviceId::new("memory-actual-receiver");
        let (receiver_out, sender_in) = pair(1, [1; 32], [2; 32]);
        let (sender_out, receiver_in) = pair(2, [2; 32], [1; 32]);
        let allowed = Arc::new(AtomicBool::new(true));
        let sender_dials = Arc::new(AtomicUsize::new(0));
        let receiver_dials = Arc::new(AtomicUsize::new(0));
        let sender = ConnectivityManager::new(
            Arc::new(MemoryBackend {
                target: receiver_id,
                connection: sender_out,
                dials: sender_dials.clone(),
            }),
            Arc::new(Members {
                peer: receiver_id,
                key: [2; 32],
                allowed: allowed.clone(),
            }),
            transfer_progress_connectivity_policy(Arc::new(SystemElapsedClock)),
        );
        let receiver = ConnectivityManager::new(
            Arc::new(MemoryBackend {
                target: sender_id,
                connection: receiver_out,
                dials: receiver_dials.clone(),
            }),
            Arc::new(Members {
                peer: sender_id,
                key: [1; 32],
                allowed: allowed.clone(),
            }),
            transfer_progress_connectivity_policy(Arc::new(SystemElapsedClock)),
        );
        let at_sender = sender.accept(sender_in).await?;
        let at_receiver = receiver.accept(receiver_in).await?;
        Ok(Self {
            sender_id,
            receiver_id,
            sender,
            receiver,
            at_sender,
            at_receiver,
            allowed,
            sender_dials,
            receiver_dials,
        })
    }
    pub async fn close(self) -> anyhow::Result<()> {
        drop(self.at_sender);
        drop(self.at_receiver);
        self.sender.shutdown().await?;
        self.receiver.shutdown().await?;
        Ok(())
    }
}
