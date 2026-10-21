//! 第二个明确的内存 backend 只验证完整 port、总预算与资源所有权；真实 QUIC 另验收。
use super::*;
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectionProtocol, ConnectivityBackend,
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation,
    PeerConnection, PeerReceiveStream, PeerSendStream, SessionConnectionAdmissionPort, StreamPair,
};
use crate::deps::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{oneshot, Notify, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::clipboard::ActiveClipboardState;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{
    ActiveClipboardDispatchError, ActiveClipboardDispatchPort, ActiveClipboardPullClientError,
    ActiveClipboardPullClientPort, ActiveClipboardReceiverPort,
};

struct MemorySend(tokio::io::Sink);
impl AsyncWrite for MemorySend {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
impl PeerSendStream for MemorySend {
    fn id(&self) -> u64 {
        0
    }
    fn reset(&mut self, _code: u64) -> Result<(), ConnectivityError> {
        // 此旧协议 fixture 不提供显式停止能力；需要停止的用例使用真实 bridge。
        Err(ConnectivityError::Stopped)
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async { Ok(()) })
    }
}
struct MemoryReceive(tokio::io::Empty);
impl AsyncRead for MemoryReceive {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, bytes)
    }
}
impl PeerReceiveStream for MemoryReceive {
    fn id(&self) -> u64 {
        0
    }
    fn stop(&mut self, _code: u64) -> Result<(), ConnectivityError> {
        Err(ConnectivityError::Stopped)
    }
}
fn streams() -> StreamPair {
    (
        Box::new(MemorySend(tokio::io::sink())),
        Box::new(MemoryReceive(tokio::io::empty())),
    )
}
struct MemoryConnection {
    id: usize,
    protocol: ConnectionProtocol,
    closed: AtomicBool,
    ended: Notify,
    open_delay: Duration,
}
impl MemoryConnection {
    fn new(id: usize, protocol: ConnectionProtocol, open_delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            id,
            protocol,
            closed: AtomicBool::new(false),
            ended: Notify::new(),
            open_delay,
        })
    }
}
#[async_trait]
impl PeerConnection for MemoryConnection {
    fn id(&self) -> usize {
        self.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [7; 32]
    }
    fn mode(&self) -> ConnectionMode {
        ConnectionMode::SingleExchange
    }
    fn protocol(&self) -> ConnectionProtocol {
        self.protocol
    }
    fn channel(&self) -> uc_core::ports::ConnectionChannel {
        uc_core::ports::ConnectionChannel::Direct
    }
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.ended.notify_waiters();
    }
    async fn closed(&self) {
        let ended = self.ended.notified();
        if !self.is_closed() {
            ended.await;
        }
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        tokio::time::sleep(self.open_delay).await;
        Ok(streams())
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        Ok(streams())
    }
}
struct MemoryBackend {
    calls: AtomicUsize,
    delay: Duration,
    open_delay: Duration,
    protocol: ConnectionProtocol,
}
#[async_trait]
impl ConnectivityBackend for MemoryBackend {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let id = self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        Ok(MemoryConnection::new(id, self.protocol, self.open_delay))
    }
}
struct FixedAdmission;
struct FixedGrant;
#[async_trait]
impl AdmissionGrant for FixedGrant {
    fn peer(&self) -> DeviceId {
        DeviceId::new("peer")
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
}
#[async_trait]
impl AdmissionAttempt for FixedAdmission {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        _: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        Ok(Box::new(FixedGrant))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for FixedAdmission {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(FixedAdmission)
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        reservation.identified(FixedGrant.peer())?;
        Ok(Box::new(FixedGrant))
    }
}
fn manager(
    protocol: ConnectionProtocol,
    connect_delay: Duration,
    open_delay: Duration,
) -> (ConnectivityManager, Arc<MemoryBackend>) {
    let backend = Arc::new(MemoryBackend {
        calls: AtomicUsize::new(0),
        delay: connect_delay,
        open_delay,
        protocol,
    });
    (
        ConnectivityManager::new(
            backend.clone(),
            Arc::new(FixedAdmission),
            ConnectivityPolicy {
                inbound_per_connection: 1,
                ..ConnectivityPolicy::default()
            },
        ),
        backend,
    )
}
struct MemoryWire {
    delay: Duration,
    responses: Mutex<Vec<ActivePullResponse>>,
}
impl MemoryWire {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            delay,
            responses: Mutex::new(vec![]),
        })
    }
}
#[async_trait]
impl ActiveClipboardWirePort for MemoryWire {
    async fn write_state(
        &self,
        _: &mut dyn PeerSendStream,
        _: &ActiveClipboardState,
    ) -> Result<(), ActiveClipboardTransportError> {
        tokio::time::sleep(self.delay).await;
        Ok(())
    }
    async fn read_state(
        &self,
        _: &mut dyn PeerReceiveStream,
    ) -> Result<ActiveStateMessage, ActiveClipboardTransportError> {
        tokio::time::sleep(self.delay).await;
        Ok(ActiveStateMessage {
            snapshot_hash: "hash".into(),
            entry_id: "remote-only".into(),
            activated_at_ms: 1,
            activated_by: "origin".into(),
        })
    }
    async fn write_pull_request(
        &self,
        _: &mut dyn PeerSendStream,
        _: &str,
    ) -> Result<(), ActiveClipboardTransportError> {
        tokio::time::sleep(self.delay).await;
        Ok(())
    }
    async fn read_pull_request(
        &self,
        _: &mut dyn PeerReceiveStream,
    ) -> Result<String, ActiveClipboardTransportError> {
        tokio::time::sleep(self.delay).await;
        Ok("hash".into())
    }
    async fn write_pull_response(
        &self,
        _: &mut dyn PeerSendStream,
        response: ActivePullResponse,
    ) -> Result<(), ActiveClipboardTransportError> {
        self.responses.lock().unwrap().push(response);
        Ok(())
    }
    async fn read_pull_response(
        &self,
        _: &mut dyn PeerReceiveStream,
    ) -> Result<ActivePullResponse, ActiveClipboardTransportError> {
        tokio::time::sleep(self.delay).await;
        Ok(ActivePullResponse::Envelope(b"opaque-envelope".to_vec()))
    }
}
struct AvailablePreparation;
#[async_trait]
impl ActivePullPreparationPort for AvailablePreparation {
    async fn prepare(
        &self,
        _: &str,
        stop: &ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        stop.check()?;
        Ok(b"opaque-envelope".to_vec())
    }
}
struct AllowContent;
#[async_trait]
impl CurrentSpaceMemberScopePort for AllowContent {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: vec![DeviceId::new("peer")],
            paused_peer_devices: vec![],
        })
    }
}
struct InlineLocalWork;
#[async_trait]
impl ActivePullLocalWorkPort for InlineLocalWork {
    async fn execute(
        &self,
        work: ActivePullWork,
        stop: ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        stop.check()?;
        work.await
    }
}
fn state() -> ActiveClipboardState {
    ActiveClipboardState::new(
        "hash",
        EntryId::from("local-only"),
        1,
        DeviceId::new("origin"),
    )
}

#[tokio::test(start_paused = true)]
async fn all_four_complete_ports_work_with_explicit_non_iroh_backend() {
    let (states, _) = manager(
        ConnectionProtocol::ActiveClipboardState,
        Duration::ZERO,
        Duration::ZERO,
    );
    let wire = MemoryWire::new(Duration::ZERO);
    ActiveStateSender::new(states.clone(), wire.clone())
        .dispatch(&DeviceId::new("peer"), &state())
        .await
        .unwrap();
    let receiver = ActiveStateReceiver::new(states.clone(), wire.clone());
    let mut observations = receiver.subscribe();
    receiver
        .receive(
            MemoryConnection::new(10, ConnectionProtocol::ActiveClipboardState, Duration::ZERO),
            Instant::now() + ACTIVE_EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap();
    let observation = observations.recv().await.unwrap();
    assert_eq!(observation.peer_device_id, DeviceId::new("peer"));
    assert_eq!(
        observation.activated_by,
        DeviceId::new("origin"),
        "原激活者不要求等于当前转发者"
    );
    let (pulls, _) = manager(
        ConnectionProtocol::ActiveClipboardPull,
        Duration::ZERO,
        Duration::ZERO,
    );
    assert_eq!(
        ActivePullClient::new(pulls.clone(), wire.clone())
            .pull(&DeviceId::new("peer"), "hash")
            .await
            .unwrap(),
        b"opaque-envelope"
    );
    let receiver = ActivePullReceiver::new(
        pulls.clone(),
        wire.clone(),
        Arc::new(AvailablePreparation),
        Arc::new(AllowContent),
        Arc::new(InlineLocalWork),
    );
    receiver
        .receive(
            MemoryConnection::new(11, ConnectionProtocol::ActiveClipboardPull, Duration::ZERO),
            Instant::now() + ACTIVE_EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap();
    assert!(
        matches!(wire.responses.lock().unwrap().as_slice(), [ActivePullResponse::Envelope(bytes)] if bytes == b"opaque-envelope")
    );
    states.shutdown().await.unwrap();
    pulls.shutdown().await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn dispatch_and_pull_do_not_renew_budget_between_dial_open_and_write() {
    for protocol in [
        ConnectionProtocol::ActiveClipboardState,
        ConnectionProtocol::ActiveClipboardPull,
    ] {
        let (manager, backend) = manager(protocol, Duration::from_secs(4), Duration::from_secs(4));
        let wire = MemoryWire::new(Duration::from_secs(4));
        let started = Instant::now();
        if protocol == ConnectionProtocol::ActiveClipboardState {
            assert!(matches!(
                ActiveStateSender::new(manager.clone(), wire)
                    .dispatch(&DeviceId::new("peer"), &state())
                    .await,
                Err(ActiveClipboardDispatchError::Io(_))
            ));
        } else {
            assert!(matches!(
                ActivePullClient::new(manager.clone(), wire)
                    .pull(&DeviceId::new("peer"), "hash")
                    .await,
                Err(ActiveClipboardPullClientError::Unreachable)
            ));
        }
        assert_eq!(started.elapsed(), ACTIVE_EXCHANGE_TIMEOUT);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1, "请求不重送");
        manager.shutdown().await.unwrap();
    }
}
#[tokio::test(start_paused = true)]
async fn inbound_state_and_pull_share_entry_deadline_with_read_and_preparation() {
    let wire = MemoryWire::new(Duration::from_secs(11));
    let (states, _) = manager(
        ConnectionProtocol::ActiveClipboardState,
        Duration::ZERO,
        Duration::ZERO,
    );
    let receiver = ActiveStateReceiver::new(states.clone(), wire.clone());
    let started = Instant::now();
    assert!(receiver
        .receive(
            MemoryConnection::new(12, ConnectionProtocol::ActiveClipboardState, Duration::ZERO),
            started + ACTIVE_EXCHANGE_TIMEOUT
        )
        .await
        .is_err());
    assert_eq!(started.elapsed(), ACTIVE_EXCHANGE_TIMEOUT);
    let (pulls, _) = manager(
        ConnectionProtocol::ActiveClipboardPull,
        Duration::ZERO,
        Duration::ZERO,
    );
    let receiver = ActivePullReceiver::new(
        pulls.clone(),
        wire,
        Arc::new(AvailablePreparation),
        Arc::new(AllowContent),
        Arc::new(InlineLocalWork),
    );
    let started = Instant::now();
    assert!(receiver
        .receive(
            MemoryConnection::new(13, ConnectionProtocol::ActiveClipboardPull, Duration::ZERO),
            started + ACTIVE_EXCHANGE_TIMEOUT
        )
        .await
        .is_err());
    assert_eq!(started.elapsed(), ACTIVE_EXCHANGE_TIMEOUT);
    states.shutdown().await.unwrap();
    pulls.shutdown().await.unwrap();
}
struct HeldPreparation {
    started: AtomicUsize,
    completed: AtomicUsize,
    release: Semaphore,
}
#[async_trait]
impl ActivePullPreparationPort for HeldPreparation {
    async fn prepare(
        &self,
        _: &str,
        stop: &ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.release.acquire().await.unwrap().forget();
        self.completed.fetch_add(1, Ordering::SeqCst);
        stop.check()?;
        Ok(b"opaque-envelope".to_vec())
    }
}
struct RetainingLocalWork(Mutex<Vec<tokio::task::JoinHandle<()>>>);
#[async_trait]
impl ActivePullLocalWorkPort for RetainingLocalWork {
    async fn execute(
        &self,
        work: ActivePullWork,
        stop: ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        stop.check()?;
        let (send, result) = oneshot::channel();
        self.0.lock().unwrap().push(tokio::spawn(async move {
            let _ = send.send(work.await);
        }));
        result.await.unwrap()
    }
}
#[tokio::test(start_paused = true)]
async fn cancelled_pull_work_keeps_original_quota_while_state_receives_independently() {
    let (pulls, _) = manager(
        ConnectionProtocol::ActiveClipboardPull,
        Duration::ZERO,
        Duration::ZERO,
    );
    let wire = MemoryWire::new(Duration::ZERO);
    let preparation = Arc::new(HeldPreparation {
        started: AtomicUsize::new(0),
        completed: AtomicUsize::new(0),
        release: Semaphore::new(0),
    });
    let local = Arc::new(RetainingLocalWork(Mutex::new(vec![])));
    let receiver = Arc::new(ActivePullReceiver::new(
        pulls.clone(),
        wire.clone(),
        preparation.clone(),
        Arc::new(AllowContent),
        local.clone(),
    ));
    let mut connections = vec![];
    let mut waiting = vec![];
    for id in 0..16 {
        let connection = MemoryConnection::new(
            100 + id,
            ConnectionProtocol::ActiveClipboardPull,
            Duration::ZERO,
        );
        connections.push(connection.clone());
        let receiver = receiver.clone();
        waiting.push(tokio::spawn(async move {
            receiver
                .receive(connection, Instant::now() + ACTIVE_EXCHANGE_TIMEOUT)
                .await
        }));
    }
    while preparation.started.load(Ordering::SeqCst) < 16 {
        tokio::task::yield_now().await;
    }
    for connection in connections {
        connection.close();
    }
    for waiter in waiting {
        assert!(waiter.await.unwrap().is_err());
    }
    assert_eq!(preparation.completed.load(Ordering::SeqCst), 0);
    let seventeenth =
        MemoryConnection::new(117, ConnectionProtocol::ActiveClipboardPull, Duration::ZERO);
    let next_connection = seventeenth.clone();
    let next_receiver = receiver.clone();
    let next = tokio::spawn(async move {
        next_receiver
            .receive(next_connection, Instant::now() + ACTIVE_EXCHANGE_TIMEOUT)
            .await
    });
    tokio::task::yield_now().await;
    let (states, _) = manager(
        ConnectionProtocol::ActiveClipboardState,
        Duration::ZERO,
        Duration::ZERO,
    );
    let state_receiver = ActiveStateReceiver::new(states.clone(), wire);
    let mut observations = state_receiver.subscribe();
    state_receiver
        .receive(
            MemoryConnection::new(
                200,
                ConnectionProtocol::ActiveClipboardState,
                Duration::ZERO,
            ),
            Instant::now() + ACTIVE_EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(observations.recv().await.unwrap().snapshot_hash, "hash");
    assert_eq!(
        preparation.started.load(Ordering::SeqCst),
        16,
        "取消网络不能提前释放十六份本地额度"
    );
    preparation.release.add_permits(16);
    while preparation.started.load(Ordering::SeqCst) < 17 {
        tokio::task::yield_now().await;
    }
    assert_eq!(preparation.completed.load(Ordering::SeqCst), 16);
    seventeenth.close();
    assert!(next.await.unwrap().is_err());
    preparation.release.add_permits(1);
    let tasks = std::mem::take(&mut *local.0.lock().unwrap());
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(preparation.completed.load(Ordering::SeqCst), 17);
    states.shutdown().await.unwrap();
    pulls.shutdown().await.unwrap();
}

struct SlowPreparation;
#[async_trait]
impl ActivePullPreparationPort for SlowPreparation {
    async fn prepare(
        &self,
        _: &str,
        _: &ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        tokio::time::sleep(Duration::from_secs(8)).await;
        Ok(vec![1])
    }
}
#[tokio::test(start_paused = true)]
async fn request_read_and_preparation_wait_use_one_inbound_budget() {
    let (pulls, _) = manager(
        ConnectionProtocol::ActiveClipboardPull,
        Duration::ZERO,
        Duration::ZERO,
    );
    let wire = MemoryWire::new(Duration::from_secs(4));
    let receiver = ActivePullReceiver::new(
        pulls.clone(),
        wire.clone(),
        Arc::new(SlowPreparation),
        Arc::new(AllowContent),
        Arc::new(InlineLocalWork),
    );
    let started = Instant::now();
    assert!(receiver
        .receive(
            MemoryConnection::new(300, ConnectionProtocol::ActiveClipboardPull, Duration::ZERO),
            started + ACTIVE_EXCHANGE_TIMEOUT
        )
        .await
        .is_err());
    assert_eq!(started.elapsed(), ACTIVE_EXCHANGE_TIMEOUT);
    assert!(
        wire.responses.lock().unwrap().is_empty(),
        "expired preparation cannot send a response"
    );
    pulls.shutdown().await.unwrap();
}

struct UnavailableScope;
#[async_trait]
impl CurrentSpaceMemberScopePort for UnavailableScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Err(CurrentSpaceMemberScopeError::recovery_required_from(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ))
    }
}
#[tokio::test(start_paused = true)]
async fn scope_failure_keeps_original_source_and_original_no_content_wire_response() {
    use std::error::Error;
    let (pulls, _) = manager(
        ConnectionProtocol::ActiveClipboardPull,
        Duration::ZERO,
        Duration::ZERO,
    );
    let wire = MemoryWire::new(Duration::ZERO);
    let receiver = ActivePullReceiver::new(
        pulls.clone(),
        wire.clone(),
        Arc::new(AvailablePreparation),
        Arc::new(UnavailableScope),
        Arc::new(InlineLocalWork),
    );
    let error = receiver
        .receive(
            MemoryConnection::new(301, ConnectionProtocol::ActiveClipboardPull, Duration::ZERO),
            Instant::now() + ACTIVE_EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        wire.responses.lock().unwrap().as_slice(),
        [ActivePullResponse::NotAvailable]
    ));
    let mut source: &dyn Error = &error;
    loop {
        if let Some(io) = source.downcast_ref::<std::io::Error>() {
            assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
            break;
        }
        source = source.source().expect("scope source must remain reachable");
    }
    pulls.shutdown().await.unwrap();
}
