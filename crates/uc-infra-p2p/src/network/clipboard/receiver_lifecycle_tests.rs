//! 用替代传输确定性验证流准入、兄弟流取消及有界回收。
use super::{ClipboardDispatchAdapter, ClipboardReceiverAdapter};
use async_trait::async_trait;
use bytes::Bytes;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf, ReadHalf, WriteHalf,
};
use tokio::sync::{mpsc, watch, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uc_application::deps::ConnectionProtocol;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ClipboardReceiverPort, ConnectionMode, ConnectivityBackend,
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation,
    PeerConnection, PeerReceiveStream, PeerSendStream, SessionConnectionAdmissionPort, StreamPair,
};
use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchPort, ClipboardHeader, ConnectionChannel, DispatchAck,
    InboundClipboardDisposition, SyncPayload,
};
use uc_sync_protocol::clipboard::{self as wire, AckCode};

struct ReceiveHalf(ReadHalf<DuplexStream>);
impl AsyncRead for ReceiveHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}
impl PeerReceiveStream for ReceiveHalf {
    fn id(&self) -> u64 {
        0
    }
    fn stop(&mut self, _code: u64) -> Result<(), ConnectivityError> {
        Err(ConnectivityError::Stopped)
    }
}
struct SendHalf {
    write: WriteHalf<DuplexStream>,
    stalled: bool,
    dropped: Arc<AtomicUsize>,
}
impl AsyncWrite for SendHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.stalled {
            Poll::Pending
        } else {
            Pin::new(&mut self.write).poll_write(cx, bytes)
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_shutdown(cx)
    }
}
impl PeerSendStream for SendHalf {
    fn id(&self) -> u64 {
        0
    }
    fn reset(&mut self, _code: u64) -> Result<(), ConnectivityError> {
        // 此旧协议 fixture 不提供显式停止能力；需要停止的用例使用真实 bridge。
        Err(ConnectivityError::Stopped)
    }
    fn settled(&mut self) -> futures_util::future::BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async { Ok(()) })
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        Ok(())
    }
}
impl Drop for SendHalf {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
struct MemoryConnection {
    id: usize,
    incoming: Mutex<mpsc::Receiver<StreamPair>>,
    closed: watch::Sender<bool>,
    accepted: AtomicUsize,
}
#[async_trait]
impl PeerConnection for MemoryConnection {
    fn id(&self) -> usize {
        self.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [0; 32]
    }
    fn mode(&self) -> ConnectionMode {
        ConnectionMode::Multiplexed
    }
    fn protocol(&self) -> ConnectionProtocol {
        ConnectionProtocol::MultiplexedClipboard
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
        let mut rx = self.closed.subscribe();
        while !*rx.borrow_and_update() {
            rx.changed().await.unwrap();
        }
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        Err(ConnectivityError::Offline)
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        let pair = self
            .incoming
            .lock()
            .await
            .recv()
            .await
            .ok_or(ConnectivityError::Stopped)?;
        self.accepted.fetch_add(1, Ordering::SeqCst);
        Ok(pair)
    }
}
struct MemoryBackend;
#[async_trait]
impl ConnectivityBackend for MemoryBackend {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        Err(ConnectivityError::Offline)
    }
}
struct Admission {
    calls: AtomicUsize,
    /// 第几次复核返回“已不再准入”；0 表示从不拒绝。
    deny_at: AtomicUsize,
    block_at: usize,
    blocked: AtomicBool,
    gate: Semaphore,
}
struct Grant(Arc<Admission>);
struct InboundAdmission(Arc<Admission>);
struct MemoryAttempt(Arc<Admission>);
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        DeviceId::new("memory-peer")
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        let call = self.0.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.0.deny_at.load(Ordering::SeqCst) {
            return Err(ConnectivityError::Denied);
        }
        if call == self.0.block_at {
            self.0.blocked.store(true, Ordering::SeqCst);
            self.0.gate.acquire().await.unwrap().forget();
        }
        Ok(())
    }
}
#[async_trait]
impl AdmissionAttempt for MemoryAttempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        _: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        Ok(Box::new(Grant(self.0.clone())))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for InboundAdmission {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(MemoryAttempt(self.0.clone()))
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        reservation.identified(Grant(self.0.clone()).peer())?;
        Ok(Box::new(Grant(self.0.clone())))
    }
}
fn receiver(block_at: usize, global: usize) -> (Arc<ClipboardReceiverAdapter>, Arc<Admission>) {
    let admission = Arc::new(Admission {
        calls: AtomicUsize::new(0),
        deny_at: AtomicUsize::new(0),
        block_at,
        blocked: AtomicBool::new(false),
        gate: Semaphore::new(0),
    });
    let manager = ConnectivityManager::new(
        Arc::new(MemoryBackend),
        Arc::new(InboundAdmission(admission.clone())),
        ConnectivityPolicy {
            inbound_per_session: global,
            ..ConnectivityPolicy::default()
        },
    );
    (Arc::new(ClipboardReceiverAdapter::new(manager)), admission)
}
fn connection_pair(id: usize) -> (Arc<MemoryConnection>, mpsc::Sender<StreamPair>) {
    let (tx, rx) = mpsc::channel(4);
    (
        Arc::new(MemoryConnection {
            id,
            incoming: Mutex::new(rx),
            closed: watch::channel(false).0,
            accepted: AtomicUsize::new(0),
        }),
        tx,
    )
}
async fn stream(
    tx: &mpsc::Sender<StreamPair>,
    payload: &'static [u8],
    stalled: bool,
) -> (DuplexStream, Arc<AtomicUsize>) {
    let (mut client, server) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(server);
    let dropped = Arc::new(AtomicUsize::new(0));
    tx.send((
        Box::new(SendHalf {
            write,
            stalled,
            dropped: dropped.clone(),
        }),
        Box::new(ReceiveHalf(read)),
    ))
    .await
    .unwrap();
    wire::write_frame(
        &mut client,
        &ClipboardHeader {
            source_entry: None,
            version: ClipboardHeader::CURRENT_VERSION,
            snapshot_hash: "a".repeat(64),
            captured_at_ms: 0,
            origin_device_id: "memory-peer".into(),
            origin_device_name: "Memory".into(),
            payload_version: 3,
        },
        None,
        &Bytes::from_static(payload),
    )
    .await
    .unwrap();
    client.shutdown().await.unwrap();
    (client, dropped)
}
fn run(
    receiver: &Arc<ClipboardReceiverAdapter>,
    connection: Arc<MemoryConnection>,
) -> tokio::task::JoinHandle<()> {
    let receiver = receiver.clone();
    tokio::spawn(async move { receiver.receive(connection).await })
}
async fn spin_until(mut ready: impl FnMut() -> bool) {
    for _ in 0..100 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        ready(),
        "controlled transport did not reach the expected stage"
    );
}

#[tokio::test]
async fn idle_peer_does_not_reserve_session_exchange_capacity() {
    let (receiver, _) = receiver(usize::MAX, 1);
    let mut deliveries = receiver.subscribe();
    let (idle, _idle_tx) = connection_pair(1);
    let idle_task = run(&receiver, idle.clone());
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let (active, tx) = connection_pair(2);
    let active_task = run(&receiver, active.clone());
    let (mut client, _) = stream(&tx, b"active", false).await;
    let delivery = tokio::time::timeout(Duration::from_millis(100), deliveries.recv())
        .await
        .expect("idle peer must not block a different peer's frame")
        .unwrap();
    delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    assert_eq!(client.read_u8().await.unwrap(), AckCode::Accepted.as_byte());
    idle.close();
    active.close();
    idle_task.await.unwrap();
    active_task.await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test]
async fn sibling_completion_preserves_an_accepted_stream_waiting_for_admission() {
    let (receiver, admission) = receiver(3, 2);
    let mut deliveries = receiver.subscribe();
    let (connection, tx) = connection_pair(1);
    let task = run(&receiver, connection.clone());
    let (mut first, _) = stream(&tx, b"first", false).await;
    let first_delivery = deliveries.recv().await.unwrap();
    let (mut second, second_dropped) = stream(&tx, b"second", false).await;
    spin_until(|| admission.blocked.load(Ordering::SeqCst)).await;
    first_delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    assert_eq!(first.read_u8().await.unwrap(), AckCode::Accepted.as_byte());
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        second_dropped.load(Ordering::SeqCst),
        0,
        "completing a sibling must not reset the accepted stream"
    );
    admission.gate.add_permits(1);
    let second_delivery = tokio::time::timeout(Duration::from_millis(100), deliveries.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_delivery.message.ciphertext.as_ref(), b"second");
    second_delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Duplicate);
    assert_eq!(
        second.read_u8().await.unwrap(),
        AckCode::DuplicateIgnored.as_byte()
    );
    connection.close();
    task.await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn ack_flow_control_stall_reclaims_exchange_capacity() {
    let (receiver, _) = receiver(usize::MAX, 1);
    let mut deliveries = receiver.subscribe();
    let (connection, tx) = connection_pair(1);
    let task = run(&receiver, connection.clone());
    let (_client, dropped) = stream(&tx, b"first", true).await;
    let delivery = deliveries.recv().await.unwrap();
    delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(61)).await;
    spin_until(|| dropped.load(Ordering::SeqCst) == 1).await;
    let (healthy, tx) = connection_pair(2);
    let healthy_task = run(&receiver, healthy.clone());
    let (mut client, _) = stream(&tx, b"healthy", false).await;
    let delivery = deliveries.recv().await.unwrap();
    assert_eq!(delivery.message.ciphertext.as_ref(), b"healthy");
    delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    assert_eq!(client.read_u8().await.unwrap(), AckCode::Accepted.as_byte());
    healthy.close();
    healthy_task.await.unwrap();
    connection.close();
    task.await.unwrap();
    receiver.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stalled_payload_admission_reclaims_exchange_capacity() {
    let (receiver, admission) = receiver(2, 1);
    let mut deliveries = receiver.subscribe();
    let (connection, tx) = connection_pair(1);
    let task = run(&receiver, connection.clone());
    let (_client, dropped) = stream(&tx, b"first", false).await;
    spin_until(|| admission.blocked.load(Ordering::SeqCst)).await;
    tokio::time::advance(Duration::from_secs(61)).await;
    spin_until(|| dropped.load(Ordering::SeqCst) == 1).await;
    assert!(deliveries.try_recv().is_err());
    let (healthy, tx) = connection_pair(2);
    let healthy_task = run(&receiver, healthy.clone());
    let (mut client, _) = stream(&tx, b"healthy", false).await;
    let delivery = deliveries.recv().await.unwrap();
    assert_eq!(delivery.message.ciphertext.as_ref(), b"healthy");
    delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    assert_eq!(client.read_u8().await.unwrap(), AckCode::Accepted.as_byte());
    healthy.close();
    healthy_task.await.unwrap();
    connection.close();
    task.await.unwrap();
    receiver.shutdown().await.unwrap();
}

struct LoopbackConnection {
    incoming: mpsc::Sender<StreamPair>,
    closed: watch::Sender<bool>,
}
#[async_trait]
impl PeerConnection for LoopbackConnection {
    fn id(&self) -> usize {
        100
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [0; 32]
    }
    fn mode(&self) -> ConnectionMode {
        ConnectionMode::Multiplexed
    }
    fn protocol(&self) -> ConnectionProtocol {
        ConnectionProtocol::MultiplexedClipboard
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
        let mut rx = self.closed.subscribe();
        while !*rx.borrow_and_update() {
            rx.changed().await.unwrap();
        }
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        Err(ConnectivityError::Offline)
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        let (client, server) = tokio::io::duplex(4096);
        fn halves(stream: DuplexStream) -> StreamPair {
            let (read, write) = tokio::io::split(stream);
            (
                Box::new(SendHalf {
                    write,
                    stalled: false,
                    dropped: Arc::new(AtomicUsize::new(0)),
                }),
                Box::new(ReceiveHalf(read)),
            )
        }
        self.incoming.send(halves(server)).await.unwrap();
        Ok(halves(client))
    }
}
struct LoopbackBackend(Arc<LoopbackConnection>);
#[async_trait]
impl ConnectivityBackend for LoopbackBackend {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn unchanged_clipboard_exchange_runs_on_a_second_backend() {
    let (receiver, admission) = receiver(usize::MAX, 2);
    let mut deliveries = receiver.subscribe();
    let (server, incoming) = connection_pair(1);
    let receiver_task = run(&receiver, server.clone());
    let dispatch = ClipboardDispatchAdapter::new(ConnectivityManager::new(
        Arc::new(LoopbackBackend(Arc::new(LoopbackConnection {
            incoming,
            closed: watch::channel(false).0,
        }))),
        Arc::new(InboundAdmission(admission)),
        ConnectivityPolicy::default(),
    ));
    let sending = tokio::spawn(async move {
        dispatch
            .dispatch(
                &DeviceId::new("memory-peer"),
                &ClipboardHeader {
                    source_entry: None,
                    version: ClipboardHeader::CURRENT_VERSION,
                    snapshot_hash: "b".repeat(64),
                    captured_at_ms: 0,
                    origin_device_id: "memory-peer".into(),
                    origin_device_name: "Memory".into(),
                    payload_version: 3,
                },
                SyncPayload {
                    ciphertext: Bytes::from_static(b"second-backend"),
                },
            )
            .await
    });
    let delivery = deliveries.recv().await.unwrap();
    assert_eq!(delivery.message.ciphertext.as_ref(), b"second-backend");
    assert!(
        !sending.is_finished(),
        "transport receipt must await durable application acceptance"
    );
    delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Duplicate);
    assert_eq!(
        sending.await.unwrap().outcome.unwrap(),
        DispatchAck::DuplicateIgnored
    );
    server.close();
    receiver_task.await.unwrap();
    receiver.shutdown().await.unwrap();
}

/// 收完内容后的准入复核失败：这次接收已经开始，必须有且只有一条带结果的完成记录，
/// 不能静默消失（旧实现用 `?` 提前返回，调用方又丢弃结果，连接上留下一个没有结局的 span）。
#[tokio::test]
async fn failed_admission_check_after_the_payload_completes_the_receive_once() {
    use opentelemetry::logs::AnyValue;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Layer;

    let exporter = InMemoryLogExporter::default();
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber =
        tracing_subscriber::registry().with(OpenTelemetryTracingBridge::new(&logs).with_filter(
            filter_fn(|metadata| matches!(metadata.target(), "uc.telemetry" | "uc.connectivity")),
        ));
    let _guard = tracing::subscriber::set_default(subscriber);

    let (receiver, admission) = receiver(usize::MAX, 4);
    // 第 1 次复核是连接准入，第 2 次才是收完内容后的复核。
    admission.deny_at.store(2, Ordering::SeqCst);
    let mut deliveries = receiver.subscribe();
    let (connection, tx) = connection_pair(1);
    let task = run(&receiver, connection.clone());
    let (_client, _dropped) = stream(&tx, b"revoked", false).await;
    spin_until(|| admission.calls.load(Ordering::SeqCst) >= 2).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    connection.close();
    task.await.unwrap();
    receiver.shutdown().await.unwrap();
    logs.force_flush().expect("flush");

    assert!(
        deliveries.try_recv().is_err(),
        "a revoked connection hands nothing to the application"
    );
    let text = |record: &opentelemetry_sdk::logs::SdkLogRecord, name: &str| {
        record
            .attributes_iter()
            .find_map(|(key, value)| match value {
                AnyValue::String(value) if key.as_str() == name => Some(value.as_str().to_owned()),
                _ => None,
            })
    };
    let completed: Vec<_> = exporter
        .get_emitted_logs()
        .unwrap()
        .into_iter()
        .filter(|entry| {
            text(&entry.record, "event.name").as_deref() == Some("uc.operation.completed")
        })
        .collect();
    assert_eq!(completed.len(), 1, "exactly one completion record");
    let record = &completed[0].record;
    assert_eq!(text(record, "error.type").as_deref(), Some("peer_rejected"));
    assert_eq!(text(record, "uc.outcome").as_deref(), Some("error"));
}
