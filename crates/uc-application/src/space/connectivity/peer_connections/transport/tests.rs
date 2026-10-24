//! 完整 owner 的第二 backend 验收；字节与真实网络另由 codec/宿主验收。
use super::{
    PeerReachabilityEnvironmentPort, PeerReachabilityTransport, PeerReachabilityTransportPorts,
    PeerReachabilityWireError, PeerReachabilityWirePort,
};
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectionProtocol, ConnectivityBackend,
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation,
    PeerConnection, PeerReceiveStream, PeerSendStream, SessionConnectionAdmissionPort, StreamPair,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{broadcast, watch, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::ports::{ClockPort, ConnectionChannel, PeerReachabilityPort, ReachabilityState};

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
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
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
fn peer(index: u8) -> DeviceId {
    DeviceId::new(format!("peer-{index}"))
}
struct MemoryConnection {
    id: usize,
    peer: u8,
    mode: ConnectionMode,
    closed: watch::Sender<bool>,
    first: AtomicBool,
    accepts: AtomicUsize,
}
impl MemoryConnection {
    fn new(id: usize, peer: u8, mode: ConnectionMode, first: bool) -> Arc<Self> {
        Arc::new(Self {
            id,
            peer,
            mode,
            closed: watch::channel(false).0,
            first: AtomicBool::new(first),
            accepts: AtomicUsize::new(0),
        })
    }
}
#[async_trait]
impl PeerConnection for MemoryConnection {
    fn id(&self) -> usize {
        self.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [self.peer; 32]
    }
    fn mode(&self) -> ConnectionMode {
        self.mode
    }
    fn protocol(&self) -> ConnectionProtocol {
        if self.mode == ConnectionMode::RetainedSingleExchange {
            ConnectionProtocol::PeerReachabilityV1
        } else {
            ConnectionProtocol::PeerReachabilityV2
        }
    }
    fn channel(&self) -> ConnectionChannel {
        ConnectionChannel::Direct
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
        if self.is_closed() {
            Err(ConnectivityError::Stopped)
        } else {
            Ok(streams())
        }
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.accepts.fetch_add(1, Ordering::SeqCst);
        if self.first.swap(false, Ordering::SeqCst) {
            return Ok(streams());
        }
        self.closed().await;
        Err(ConnectivityError::Stopped)
    }
}
struct MemoryBackend {
    mode: ConnectionMode,
    connections: Mutex<Vec<Arc<MemoryConnection>>>,
}
#[async_trait]
impl ConnectivityBackend for MemoryBackend {
    async fn connect(
        &self,
        target: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let index = target
            .as_str()
            .strip_prefix("peer-")
            .unwrap()
            .parse::<u8>()
            .unwrap();
        let mut connections = self.connections.lock().unwrap();
        let connection = MemoryConnection::new(connections.len(), index, self.mode, false);
        connections.push(connection.clone());
        Ok(connection)
    }
}
struct Authority {
    fail: AtomicBool,
    denied: AtomicBool,
    blocked: AtomicBool,
    started: AtomicUsize,
    release: Semaphore,
}
impl Default for Authority {
    fn default() -> Self {
        Self {
            fail: AtomicBool::new(false),
            denied: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            started: AtomicUsize::new(0),
            release: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl PeerAdmissionPort for Authority {
    async fn is_admitted(&self, _: &DeviceId) -> Result<bool, PeerAdmissionError> {
        if self.blocked.load(Ordering::SeqCst) {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.release.acquire().await.unwrap().forget();
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(PeerAdmissionError::ReadFailed {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "private-read-sentinel",
                )),
            });
        }
        Ok(!self.denied.load(Ordering::SeqCst))
    }
}
struct MemoryAdmission(Arc<Authority>);
struct MemoryAttempt {
    authority: Arc<Authority>,
    peer: DeviceId,
}
struct MemoryGrant {
    authority: Arc<Authority>,
    peer: DeviceId,
}
#[async_trait]
impl AdmissionGrant for MemoryGrant {
    fn peer(&self) -> DeviceId {
        self.peer
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if self
            .authority
            .is_admitted(&self.peer)
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?
        {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
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
        Ok(Box::new(MemoryGrant {
            authority: self.authority.clone(),
            peer: self.peer,
        }))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for MemoryAdmission {
    fn begin(&self, peer: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(MemoryAttempt {
            authority: self.0.clone(),
            peer,
        })
    }
    async fn adopt_inbound(
        &self,
        raw: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        reservation.identified(peer(raw.authenticated_key()[0]))?;
        Ok(Box::new(MemoryGrant {
            authority: self.0.clone(),
            peer: peer(raw.authenticated_key()[0]),
        }))
    }
}
struct RecordingEnvironment {
    forgotten: Mutex<Vec<DeviceId>>,
    failures: Mutex<Vec<DeviceId>>,
    disconnects: AtomicUsize,
}
#[async_trait]
impl PeerReachabilityEnvironmentPort for RecordingEnvironment {
    async fn identify(&self, key: &[u8; 32]) -> Result<DeviceId, ConnectivityError> {
        Ok(peer(key[0]))
    }
    async fn recover_for_demand(&self) {}
    fn forget(&self, peer: &DeviceId) {
        self.forgotten.lock().unwrap().push(*peer);
    }
    fn disconnect_all(&self) {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
    }
    fn communication_failed(&self, peer: DeviceId) {
        self.failures.lock().unwrap().push(peer);
    }
}
#[derive(Default)]
struct RecordingWire {
    operations: AtomicUsize,
    checks: AtomicUsize,
    reply_blocked: AtomicBool,
}
#[async_trait]
impl PeerReachabilityWirePort for RecordingWire {
    async fn read_admission(
        &self,
        _: &mut dyn PeerReceiveStream,
        _: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn write_admission(
        &self,
        _: &mut dyn PeerSendStream,
        _: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn read_confirmation(
        &self,
        _: &mut dyn PeerReceiveStream,
        _: Instant,
    ) -> Result<bool, PeerReachabilityWireError> {
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
    async fn write_confirmation(
        &self,
        _: &mut dyn PeerSendStream,
        _: bool,
        _: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn read_challenge(
        &self,
        _: &mut dyn PeerReceiveStream,
        _: Instant,
    ) -> Result<[u8; 16], PeerReachabilityWireError> {
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok([1; 16])
    }
    async fn write_challenge(
        &self,
        _: &mut dyn PeerSendStream,
        _: &[u8; 16],
        _: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn read_reply(
        &self,
        _: &mut dyn PeerReceiveStream,
        _: &[u8; 16],
        _: Instant,
    ) -> Result<bool, PeerReachabilityWireError> {
        if self.reply_blocked.load(Ordering::SeqCst) {
            futures::future::pending::<()>().await;
        }
        Ok(true)
    }
    async fn write_reply(
        &self,
        _: &mut dyn PeerSendStream,
        _: &[u8; 16],
        _: bool,
        _: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct FixedClock;
impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        1_700_000_000_000
    }
}
struct Fixture {
    owner: Arc<PeerReachabilityTransport>,
    backend: Arc<MemoryBackend>,
    authority: Arc<Authority>,
    wire: Arc<RecordingWire>,
    environment: Arc<RecordingEnvironment>,
}
impl Fixture {
    fn new(mode: ConnectionMode) -> Self {
        let authority = Arc::new(Authority::default());
        let backend = Arc::new(MemoryBackend {
            mode,
            connections: Mutex::new(Vec::new()),
        });
        let wire = Arc::new(RecordingWire::default());
        let environment = Arc::new(RecordingEnvironment {
            forgotten: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            disconnects: AtomicUsize::new(0),
        });
        let manager = ConnectivityManager::new(
            backend.clone(),
            Arc::new(MemoryAdmission(authority.clone())),
            ConnectivityPolicy {
                presence_retention: true,
                inbound_per_connection: 2,
                flights_per_session: 4,
                ..ConnectivityPolicy::default()
            },
        );
        let owner = PeerReachabilityTransport::new(
            manager,
            authority.clone(),
            PeerReachabilityTransportPorts {
                wire: wire.clone(),
                environment: environment.clone(),
            },
            Arc::new(FixedClock),
            broadcast::channel(64).0,
        );
        Self {
            owner,
            backend,
            authority,
            wire,
            environment,
        }
    }
}
#[tokio::test]
async fn six_members_remain_online_after_batched_attempt_capacity_is_released() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    for index in 1..=6 {
        assert_eq!(
            f.owner.verify_reachable(&peer(index)).await.unwrap(),
            ReachabilityState::Online
        );
    }
    for index in 1..=6 {
        assert_eq!(
            f.owner.current_state(&peer(index)).await,
            ReachabilityState::Online
        );
    }
    assert_eq!(f.backend.connections.lock().unwrap().len(), 6);
    assert!(f
        .backend
        .connections
        .lock()
        .unwrap()
        .iter()
        .all(|raw| !raw.is_closed()));
    f.owner.shutdown().await.unwrap();
    assert!(f
        .backend
        .connections
        .lock()
        .unwrap()
        .iter()
        .all(|raw| raw.is_closed()));
}
#[tokio::test]
async fn local_authority_read_failure_preserves_display_and_typed_source_without_new_wire_work() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    assert_eq!(
        f.owner.verify_reachable(&peer(1)).await.unwrap(),
        ReachabilityState::Online
    );
    let before = f.wire.operations.load(Ordering::SeqCst);
    f.authority.fail.store(true, Ordering::SeqCst);
    let error = f.owner.verify_reachable(&peer(1)).await.unwrap_err();
    let mut source = Some(&error as &dyn std::error::Error);
    let mut found = false;
    while let Some(error) = source {
        found |= error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
        source = error.source();
    }
    assert!(found);
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Online
    );
    assert_eq!(f.wire.operations.load(Ordering::SeqCst), before);
    assert!(!f.backend.connections.lock().unwrap()[0].is_closed());
    f.authority.fail.store(false, Ordering::SeqCst);
    f.authority.denied.store(true, Ordering::SeqCst);
    assert_eq!(
        f.owner.verify_reachable(&peer(1)).await.unwrap(),
        ReachabilityState::Offline
    );
    assert!(f.backend.connections.lock().unwrap()[0].is_closed());
    f.owner.shutdown().await.unwrap();
}
#[tokio::test]
async fn delayed_local_read_failure_preserves_the_newer_connection_closed_offline_state() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    assert_eq!(
        f.owner.verify_reachable(&peer(1)).await.unwrap(),
        ReachabilityState::Online
    );
    let mut changes = f.owner.subscribe();
    f.authority.blocked.store(true, Ordering::SeqCst);
    let owner = f.owner.clone();
    let pending = tokio::spawn(async move { owner.verify_reachable(&peer(1)).await });
    while f.authority.started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    f.backend.connections.lock().unwrap()[0].close();
    let change = tokio::time::timeout(Duration::from_secs(1), changes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(change.state, ReachabilityState::Offline);
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Offline
    );
    f.authority.fail.store(true, Ordering::SeqCst);
    f.authority.release.add_permits(1);
    assert!(pending.await.unwrap().is_err());
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Offline
    );
    assert!(changes.try_recv().is_err());
    f.owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn v1_first_round_is_retained_without_v2_challenge_or_redial() {
    let f = Fixture::new(ConnectionMode::RetainedSingleExchange);
    for _ in 0..3 {
        assert_eq!(
            f.owner.verify_reachable(&peer(1)).await.unwrap(),
            ReachabilityState::Online
        );
    }
    assert_eq!(f.backend.connections.lock().unwrap().len(), 1);
    assert_eq!(f.wire.checks.load(Ordering::SeqCst), 0);
    f.owner.shutdown().await.unwrap();
}
#[tokio::test]
async fn seventeen_unknown_connections_are_bounded_before_the_first_stream_and_shutdown_reclaims_them(
) {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    let mut jobs = Vec::new();
    let mut raws = Vec::new();
    for index in 0..16 {
        let raw = MemoryConnection::new(index, 1, ConnectionMode::Multiplexed, false);
        let owner = f.owner.clone();
        let input = raw.clone();
        jobs.push(tokio::spawn(async move {
            owner.accept(input, Instant::now()).await
        }));
        raws.push(raw);
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while raws
            .iter()
            .any(|raw| raw.accepts.load(Ordering::SeqCst) == 0)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let excess = MemoryConnection::new(17, 1, ConnectionMode::Multiplexed, false);
    assert!(f
        .owner
        .accept(excess.clone(), Instant::now())
        .await
        .is_err());
    assert!(excess.is_closed());
    f.owner.disconnect_all().await;
    for job in jobs {
        assert!(job.await.unwrap().is_err());
    }
    assert!(raws.iter().all(|raw| raw.is_closed()));
    f.owner.activate().await;
    assert_eq!(
        f.owner.verify_reachable(&peer(2)).await.unwrap(),
        ReachabilityState::Online
    );
    f.owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn caller_deadline_does_not_release_the_four_real_local_work_permits_or_start_late_wire_steps(
) {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.authority.blocked.store(true, Ordering::SeqCst);
    let mut calls = Vec::new();
    for index in 1..=4 {
        let owner = f.owner.clone();
        calls.push(tokio::spawn(async move {
            owner.verify_reachable(&peer(index)).await
        }));
    }
    while f.authority.started.load(Ordering::SeqCst) < 4 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(21)).await;
    for call in calls {
        assert!(call.await.unwrap().is_err());
    }
    assert!(f.owner.verify_reachable(&peer(5)).await.is_err());
    assert_eq!(f.authority.started.load(Ordering::SeqCst), 4);
    assert_eq!(f.wire.operations.load(Ordering::SeqCst), 0);
    f.authority.blocked.store(false, Ordering::SeqCst);
    f.authority.release.add_permits(4);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(f.wire.operations.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.owner.verify_reachable(&peer(5)).await.unwrap(),
        ReachabilityState::Online
    );
    f.owner.shutdown().await.unwrap();
}
#[tokio::test]
async fn communication_failure_only_reports_an_opportunity_and_keeps_valid_evidence_and_connection()
{
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.owner.verify_reachable(&peer(1)).await.unwrap();
    f.owner.report_communication_failure(&peer(1)).await;
    assert_eq!(*f.environment.failures.lock().unwrap(), vec![peer(1)]);
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Online
    );
    assert!(!f.backend.connections.lock().unwrap()[0].is_closed());
    f.owner.shutdown().await.unwrap();
}

/// 移除后即使资格后来重新允许，旧在途检查也不能提交新一代的 Online。
#[tokio::test]
async fn forget_and_readd_cannot_revive_an_old_authority_read() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.authority.blocked.store(true, Ordering::SeqCst);
    let owner = f.owner.clone();
    let old = tokio::spawn(async move { owner.verify_reachable(&peer(1)).await });
    while f.authority.started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    f.owner.forget(&peer(1)).await;
    f.authority.blocked.store(false, Ordering::SeqCst);
    assert_eq!(
        f.owner.verify_reachable(&peer(1)).await.unwrap(),
        ReachabilityState::Online
    );
    let operations = f.wire.operations.load(Ordering::SeqCst);
    f.authority.release.add_permits(1);
    assert_ne!(old.await.unwrap().unwrap(), ReachabilityState::Online);
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Online
    );
    assert_eq!(f.wire.operations.load(Ordering::SeqCst), operations);
    f.owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn worker_join_failure_remains_typed_at_shutdown_after_reaping() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    let (send, receive) = tokio::sync::oneshot::channel();
    f.owner.runner.spawn(async move {
        let _ = send.send(());
        panic!("private-worker-failure");
    });
    receive.await.unwrap();
    tokio::task::yield_now().await;
    // 后续真实完整动作触发已结束任务回收，不能丢失它的 source。
    f.owner.verify_reachable(&peer(1)).await.unwrap();
    let failure = f.owner.shutdown().await.unwrap_err();
    let mut source = Some(&failure as &dyn std::error::Error);
    let mut found = false;
    while let Some(error) = source {
        found |= error
            .downcast_ref::<Arc<tokio::task::JoinError>>()
            .is_some_and(|error| error.is_panic());
        source = error.source();
    }
    assert!(found);
    assert!(f
        .backend
        .connections
        .lock()
        .unwrap()
        .iter()
        .all(|connection| connection.is_closed()));
}

#[tokio::test]
async fn repeated_activate_keeps_the_current_session_and_shutdown_cannot_be_reactivated() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.owner.verify_reachable(&peer(1)).await.unwrap();
    f.owner.activate().await;
    assert_eq!(
        f.owner.verify_reachable(&peer(1)).await.unwrap(),
        ReachabilityState::Online
    );
    assert_eq!(f.backend.connections.lock().unwrap().len(), 1);
    f.owner.shutdown().await.unwrap();
    f.owner.activate().await;
    assert!(f.owner.verify_reachable(&peer(1)).await.is_err());
    assert!(f
        .backend
        .connections
        .lock()
        .unwrap()
        .iter()
        .all(|connection| connection.is_closed()));
}

/// actor 的不透明结局应覆盖装配层兜底，且本机读取错误不是远端拒绝。
#[tokio::test]
async fn opaque_observation_feedback_crosses_owner_tasks_and_finishes_once_without_sensitive_source(
) {
    use uc_observability_contract::diagnostics::connectivity::{
        ConfirmationFailure, PresenceCheckObservation, PresenceCheckResult,
    };
    use uc_testkit::log_capture::CapturedLogs;
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.owner.verify_reachable(&peer(1)).await.unwrap();
    let logs = CapturedLogs::default();
    let _logs = logs.install();
    f.authority.fail.store(true, Ordering::SeqCst);
    let observation = PresenceCheckObservation::begin();
    assert!(observation
        .scope(f.owner.verify_reachable(&peer(1)))
        .await
        .is_err());
    observation.finish(PresenceCheckResult::Confirmation(
        ConfirmationFailure::PeerNotAdmitted,
    ));
    let output = logs.output();
    assert_eq!(logs.count("presence.check.completed"), 1, "{output}");
    assert!(output.contains("authority_unavailable"), "{output}");
    assert!(!output.contains("peer_not_admitted"), "{output}");
    assert!(!output.contains("private-read-sentinel"), "{output}");
    f.owner.shutdown().await.unwrap();
}

/// 机会切换取消旧网络等待；保留连接可立即供后继核验，不继续持检查锁五秒。
#[tokio::test]
async fn cancelled_network_verification_releases_the_peer_check_without_closing_retained_connection(
) {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.owner.verify_reachable(&peer(1)).await.unwrap();
    f.wire.reply_blocked.store(true, Ordering::SeqCst);
    let old = tokio::spawn({
        let owner = f.owner.clone();
        async move { owner.verify_reachable(&peer(1)).await }
    });
    while f.wire.checks.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    old.abort();
    assert!(old.await.unwrap_err().is_cancelled());
    f.wire.reply_blocked.store(false, Ordering::SeqCst);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(200),
            f.owner.verify_reachable(&peer(1))
        )
        .await
        .unwrap()
        .unwrap(),
        ReachabilityState::Online
    );
    assert_eq!(f.backend.connections.lock().unwrap().len(), 1);
    assert!(!f.backend.connections.lock().unwrap()[0].is_closed());
    f.owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn forgotten_inbound_cannot_start_grant_work_or_send_confirmation_after_its_local_read_settles(
) {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    f.authority.blocked.store(true, Ordering::SeqCst);
    let raw = MemoryConnection::new(100, 1, ConnectionMode::Multiplexed, true);
    let incoming = tokio::spawn({
        let owner = f.owner.clone();
        let raw = raw.clone();
        async move { owner.accept(raw, Instant::now()).await }
    });
    while f.authority.started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    f.owner.forget(&peer(1)).await;
    f.authority.release.add_permits(10);
    assert!(incoming.await.unwrap().is_err());
    assert_eq!(
        f.authority.started.load(Ordering::SeqCst),
        1,
        "revoked work must not begin the next local grant step"
    );
    assert_eq!(
        f.wire.operations.load(Ordering::SeqCst),
        1,
        "only the original request was read"
    );
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Unknown
    );
    assert!(raw.is_closed());
    f.owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn third_inbound_replaces_the_oldest_promoted_connection_even_when_ids_are_not_ordered() {
    let f = Fixture::new(ConnectionMode::Multiplexed);
    let first = MemoryConnection::new(100, 1, ConnectionMode::Multiplexed, true);
    let second = MemoryConnection::new(1, 1, ConnectionMode::Multiplexed, true);
    let third = MemoryConnection::new(50, 1, ConnectionMode::Multiplexed, true);
    for connection in [&first, &second, &third] {
        f.owner
            .accept(connection.clone(), Instant::now())
            .await
            .unwrap();
    }
    assert!(
        first.is_closed(),
        "replacement follows promotion order, not native identifier order"
    );
    assert!(!second.is_closed());
    assert!(!third.is_closed());
    assert_eq!(
        f.owner.current_state(&peer(1)).await,
        ReachabilityState::Online
    );
    f.owner.shutdown().await.unwrap();
}
