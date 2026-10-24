use super::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectionProtocol, ConnectivityBackend,
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation,
    PeerConnection, PeerReceiveStream, PeerSendStream, SessionConnectionAdmissionPort, StreamPair,
};
use async_trait::async_trait;
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{Empty, Sink};
use tokio::sync::{watch, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;

impl PeerSendStream for Sink {
    fn id(&self) -> u64 {
        0
    }
    fn reset(&mut self, _code: u64) -> Result<(), ConnectivityError> {
        // 此旧协议 fixture 不提供显式停止能力；需要停止的用例使用真实 bridge。
        Err(ConnectivityError::Stopped)
    }
    fn settled(&mut self) -> futures::future::BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async { Ok(()) })
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        Ok(())
    }
}
impl PeerReceiveStream for Empty {
    fn id(&self) -> u64 {
        0
    }
    fn stop(&mut self, _code: u64) -> Result<(), ConnectivityError> {
        Err(ConnectivityError::Stopped)
    }
}
struct TestConnection {
    id: usize,
    mode: ConnectionMode,
    closed: watch::Sender<bool>,
}
#[async_trait]
impl PeerConnection for TestConnection {
    fn id(&self) -> usize {
        self.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [0; 32]
    }
    fn mode(&self) -> ConnectionMode {
        self.mode
    }
    fn protocol(&self) -> ConnectionProtocol {
        match self.mode {
            ConnectionMode::SingleExchange => ConnectionProtocol::Clipboard,
            ConnectionMode::RetainedSingleExchange => ConnectionProtocol::PeerReachabilityV1,
            ConnectionMode::Multiplexed => ConnectionProtocol::MultiplexedClipboard,
        }
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
        if self.is_closed() {
            return Err(ConnectivityError::Stopped);
        }
        Ok((Box::new(tokio::io::sink()), Box::new(tokio::io::empty())))
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.open_bi().await
    }
}
struct Backend {
    gate: Semaphore,
    calls: AtomicUsize,
    mode: ConnectionMode,
    connections: Mutex<Vec<Arc<TestConnection>>>,
}
#[async_trait]
impl ConnectivityBackend for Backend {
    async fn connect(
        &self,
        _: DeviceId,
        _address_hint: Option<Vec<u8>>,
        _cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let id = self.calls.fetch_add(1, Ordering::SeqCst);
        let permit = self.gate.acquire().await.unwrap();
        permit.forget();
        let (closed, _) = watch::channel(false);
        let connection = Arc::new(TestConnection {
            id,
            mode: self.mode,
            closed,
        });
        self.connections.lock().unwrap().push(connection.clone());
        Ok(connection)
    }
}
struct Admission(Arc<AtomicUsize>);
struct Attempt {
    epoch: usize,
    current: Arc<AtomicUsize>,
    peer: DeviceId,
}
struct Grant {
    epoch: usize,
    current: Arc<AtomicUsize>,
    peer: DeviceId,
}
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.peer
    }
    async fn can_retain(&self) -> Result<bool, ConnectivityError> {
        self.check_current().await?;
        Ok(true)
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if self.epoch == self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
#[async_trait]
impl AdmissionAttempt for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if self.epoch == self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
    async fn adopt(
        &self,
        _: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let grant = Grant {
            epoch: self.epoch,
            current: self.current.clone(),
            peer: self.peer,
        };
        grant.check_current().await?;
        Ok(Box::new(grant))
    }
}
struct InboundFence {
    epoch: usize,
    current: Arc<AtomicUsize>,
}
impl super::InboundAdmissionFence for InboundFence {
    fn check_current(&self, _: &DeviceId) -> Result<(), ConnectivityError> {
        if self.epoch == self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for Admission {
    fn inbound_fence(&self) -> Option<Box<dyn super::InboundAdmissionFence>> {
        Some(Box::new(InboundFence {
            epoch: self.0.load(Ordering::SeqCst),
            current: self.0.clone(),
        }))
    }
    fn begin(&self, peer: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            epoch: self.0.load(Ordering::SeqCst),
            current: self.0.clone(),
            peer,
        })
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        _reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        panic!("outbound fixture")
    }
}
fn fixture(
    mode: ConnectionMode,
    policy: ConnectivityPolicy,
) -> (ConnectivityManager, Arc<Backend>, Arc<AtomicUsize>) {
    let backend = Arc::new(Backend {
        gate: Semaphore::new(0),
        calls: AtomicUsize::new(0),
        mode,
        connections: Mutex::new(Vec::new()),
    });
    let epoch = Arc::new(AtomicUsize::new(0));
    let manager =
        ConnectivityManager::new(backend.clone(), Arc::new(Admission(epoch.clone())), policy);
    (manager, backend, epoch)
}

#[tokio::test]
async fn unsupported_uni_does_not_consume_single_exchange_or_capacity() {
    let (manager, backend, _) = fixture(
        ConnectionMode::SingleExchange,
        ConnectivityPolicy::default(),
    );
    let connecting = request(&manager, 2);
    backend.gate.add_permits(1);
    let lease = connecting.await.unwrap().unwrap();
    let result = lease
        .open_uni(
            Instant::now() + Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await;
    match result {
        Err(error) => {
            assert!(matches!(&error, ConnectivityError::Capability(_)));
            assert!(error.source().is_some());
        }
        Ok(_) => panic!("只支持双向流的 backend 不应伪装为单向成功"),
    }
    let stream = lease
        .open_bi(Instant::now() + Duration::from_secs(2))
        .await
        .unwrap();
    drop(stream);
    drop(lease);
    manager.shutdown().await.unwrap();
}
fn request(
    manager: &ConnectivityManager,
    seconds: u64,
) -> tokio::task::JoinHandle<Result<super::ConnectionLease, ConnectivityError>> {
    let manager = manager.clone();
    tokio::spawn(async move {
        manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(seconds),
                &CancellationToken::new(),
            )
            .await
    })
}
async fn started(backend: &Backend, calls: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while backend.calls.load(Ordering::SeqCst) < calls {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn request_created_before_external_queue_cannot_rejoin_with_a_new_epoch_or_cached_connection()
{
    let (manager, backend, epoch) =
        fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let old = manager.request(DeviceId::new("peer"));
    let copied = old.clone();
    epoch.fetch_add(2, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    for request in [&old, &copied] {
        assert!(matches!(
            request
                .connect(Instant::now() + Duration::from_secs(10), &cancel)
                .await,
            Err(ConnectivityError::Denied)
        ));
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    let fresh = request(&manager, 10);
    started(&backend, 1).await;
    backend.gate.add_permits(1);
    let connection = fresh
        .await
        .expect("fresh request task")
        .expect("current request");
    assert!(matches!(
        old.connect(Instant::now() + Duration::from_secs(10), &cancel)
            .await,
        Err(ConnectivityError::Denied)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    drop(connection);
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn request_created_before_session_close_cannot_start_after_reactivation() {
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let old = manager.request(DeviceId::new("peer"));
    manager.disconnect_all().await.expect("close session");
    let result = old
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(ConnectivityError::Stopped)));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    let fresh = request(&manager, 10);
    started(&backend, 1).await;
    backend.gate.add_permits(1);
    drop(
        fresh
            .await
            .expect("fresh task")
            .expect("reactivated session"),
    );
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn supervised_shared_dial_keeps_other_waiters_when_one_cancels_or_expires() {
    for expire in [false, true] {
        let (manager, backend, _) = fixture(
            ConnectionMode::Multiplexed,
            ConnectivityPolicy {
                supervise_local_work: true,
                ..ConnectivityPolicy::default()
            },
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let owner = manager.clone();
        let mut first = tokio::spawn(async move {
            owner
                .request(DeviceId::new("peer"))
                .connect(Instant::now() + Duration::from_secs(1), &token)
                .await
        });
        started(&backend, 1).await;
        let following = request(&manager, 10);
        tokio::task::yield_now().await;
        if expire {
            tokio::time::advance(Duration::from_secs(1)).await;
        } else {
            cancel.cancel();
        }
        tokio::task::yield_now().await;
        let std::task::Poll::Ready(result) = futures::poll!(&mut first) else {
            panic!("one waiter waited for its sibling's dial");
        };
        let Err(error) = result.expect("first waiter task") else {
            panic!("stopped waiter received a connection");
        };
        assert!(if expire {
            matches!(error, ConnectivityError::TimedOut(_))
        } else {
            matches!(error, ConnectivityError::Stopped)
        });
        assert!(!following.is_finished());
        backend.gate.add_permits(1);
        let connection = following
            .await
            .expect("sibling task")
            .expect("sibling dial preserved");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        drop(connection);
        manager.shutdown().await.expect("shutdown");
    }
}

#[tokio::test(start_paused = true)]
async fn last_waiter_keeps_capacity_and_backend_failure_until_real_settlement() {
    struct FailingBackend {
        entered: Semaphore,
        finish: Semaphore,
    }
    #[async_trait]
    impl ConnectivityBackend for FailingBackend {
        async fn connect(
            &self,
            _: DeviceId,
            _address_hint: Option<Vec<u8>>,
            _: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            self.entered.add_permits(1);
            self.finish
                .acquire()
                .await
                .expect("backend finish")
                .forget();
            Err(ConnectivityError::Backend(Box::new(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            ))))
        }
    }
    for expire in [false, true] {
        let backend = Arc::new(FailingBackend {
            entered: Semaphore::new(0),
            finish: Semaphore::new(0),
        });
        let manager = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
            ConnectivityPolicy {
                supervise_local_work: true,
                flights_per_session: 1,
                ..ConnectivityPolicy::default()
            },
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let owner = manager.clone();
        let mut first = tokio::spawn(async move {
            owner
                .request(DeviceId::new("peer"))
                .connect(Instant::now() + Duration::from_secs(1), &token)
                .await
        });
        backend
            .entered
            .acquire()
            .await
            .expect("backend entered")
            .forget();
        if expire {
            tokio::time::advance(Duration::from_secs(1)).await;
        } else {
            cancel.cancel();
        }
        tokio::task::yield_now().await;
        assert!(futures::poll!(&mut first).is_pending());
        assert!(matches!(
            manager
                .request(DeviceId::new("other"))
                .connect(
                    Instant::now() + Duration::from_secs(1),
                    &CancellationToken::new(),
                )
                .await,
            Err(ConnectivityError::Busy(_)),
        ));
        backend.finish.add_permits(1);
        let Err(ConnectivityError::Settlement {
            primary,
            additional,
        }) = first.await.expect("waiter task")
        else {
            panic!("backend failure after logical stop was lost");
        };
        assert!(if expire {
            matches!(*primary, ConnectivityError::TimedOut(_))
        } else {
            matches!(*primary, ConnectivityError::Stopped)
        });
        let ConnectivityError::Shared(source) = *additional else {
            panic!("shared source lost");
        };
        let ConnectivityError::Backend(source) = source.as_ref() else {
            panic!("backend source lost");
        };
        assert_eq!(
            source
                .downcast_ref::<std::io::Error>()
                .expect("original IO error")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        manager.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn cancelling_first_waiter_keeps_shared_dial_for_followers() {
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let first = request(&manager, 5);
    started(&backend, 1).await;
    let follower = request(&manager, 5);
    tokio::task::yield_now().await;
    first.abort();
    let _ = first.await;
    backend.gate.add_permits(1);
    let lease = follower.await.unwrap().unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    let stream = lease
        .open_bi(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    drop(lease);
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    drop(stream);
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}
#[tokio::test]
async fn final_waiter_cancellation_joins_dial_and_next_request_starts_fresh() {
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let first = request(&manager, 5);
    started(&backend, 1).await;
    first.abort();
    let _ = first.await;
    let second = request(&manager, 5);
    started(&backend, 2).await;
    backend.gate.add_permits(1);
    drop(second.await.unwrap().unwrap());
    manager.shutdown().await.unwrap();
    assert_eq!(backend.connections.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn old_protocol_gives_each_waiter_an_exclusive_connection() {
    let (manager, backend, _) = fixture(
        ConnectionMode::SingleExchange,
        ConnectivityPolicy::default(),
    );
    let first = request(&manager, 5);
    started(&backend, 1).await;
    let second = request(&manager, 5);
    tokio::task::yield_now().await;
    backend.gate.add_permits(2);
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_ne!(first.record.connection.id(), second.record.connection.id());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    drop((first, second));
    manager.shutdown().await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn deadlines_are_independent_and_wait_does_not_reset_stream_budget() {
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let first = request(&manager, 1);
    tokio::task::yield_now().await;
    let second = request(&manager, 5);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(matches!(
        first.await.unwrap(),
        Err(ConnectivityError::TimedOut(_))
    ));
    backend.gate.add_permits(1);
    let second = second.await.unwrap().unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        second
            .open_bi(Instant::now() - Duration::from_secs(1))
            .await,
        Err(ConnectivityError::DeadlineExpired)
    ));
    drop(second);
    manager.shutdown().await.unwrap();
}
#[tokio::test]
async fn waiter_capacity_is_bounded_and_cancellation_returns_capacity() {
    let policy = ConnectivityPolicy {
        waiters_per_peer: 1,
        ..ConnectivityPolicy::default()
    };
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy);
    let first = request(&manager, 5);
    started(&backend, 1).await;
    assert!(matches!(
        manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new()
            )
            .await,
        Err(ConnectivityError::Busy(_))
    ));
    first.abort();
    let _ = first.await;
    let next = request(&manager, 5);
    started(&backend, 2).await;
    backend.gate.add_permits(1);
    drop(next.await.unwrap().unwrap());
    manager.shutdown().await.unwrap();
}
#[tokio::test]
async fn revocation_during_dial_rejects_late_connection() {
    let (manager, backend, epoch) =
        fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let first = request(&manager, 5);
    started(&backend, 1).await;
    epoch.fetch_add(1, Ordering::SeqCst);
    backend.gate.add_permits(1);
    assert!(first.await.unwrap().is_err());
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}
#[tokio::test]
async fn active_streams_are_bounded_and_drop_only_releases_own_slot() {
    let policy = ConnectivityPolicy {
        outbound_per_peer: 1,
        ..ConnectivityPolicy::default()
    };
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy);
    let first = request(&manager, 5);
    backend.gate.add_permits(1);
    let lease = first.await.unwrap().unwrap();
    let stream = lease
        .open_bi(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert!(matches!(
        lease
            .open_bi(Instant::now() + Duration::from_millis(10))
            .await,
        Err(ConnectivityError::TimedOut(_))
    ));
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    drop(stream);
    drop(
        lease
            .open_bi(Instant::now() + Duration::from_secs(1))
            .await
            .unwrap(),
    );
    drop(lease);
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn single_exchange_followers_work_with_one_dial_slot() {
    let policy = ConnectivityPolicy {
        flights_per_session: 1,
        ..ConnectivityPolicy::default()
    };
    let (manager, backend, _) = fixture(ConnectionMode::SingleExchange, policy);
    let first = request(&manager, 1);
    started(&backend, 1).await;
    let second = request(&manager, 1);
    tokio::task::yield_now().await;
    backend.gate.add_permits(2);
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_ne!(first.record.connection.id(), second.record.connection.id());
    drop((first, second));
    manager.shutdown().await.unwrap();
}
#[tokio::test]
async fn shutdown_closes_leased_connections_in_both_modes() {
    for mode in [ConnectionMode::Multiplexed, ConnectionMode::SingleExchange] {
        let (manager, backend, _) = fixture(mode, ConnectivityPolicy::default());
        backend.gate.add_permits(1);
        let lease = request(&manager, 5).await.unwrap().unwrap();
        let stream = lease
            .open_bi(Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        manager.shutdown().await.unwrap();
        assert!(backend.connections.lock().unwrap()[0].is_closed());
        assert!(manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new()
            )
            .await
            .is_err());
        drop((lease, stream));
        manager.shutdown().await.unwrap();
    }
}
#[test]
fn errors_hide_private_material_while_preserving_the_source() {
    use std::error::Error;
    let error =
        ConnectivityError::Backend(Box::new(std::io::Error::other("private-address-sentinel")));
    assert!(!format!("{error:?} {error}").contains("private-address-sentinel"));
    assert!(error.source().unwrap().is::<std::io::Error>());
}
#[tokio::test]
async fn backend_panic_reaches_waiter_and_repeated_shutdown_retains_source() {
    struct Panicking;
    #[async_trait]
    impl ConnectivityBackend for Panicking {
        async fn connect(
            &self,
            _: DeviceId,
            _address_hint: Option<Vec<u8>>,
            _cancel: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            panic!("private-panic-sentinel")
        }
    }
    let manager = ConnectivityManager::new(
        Arc::new(Panicking),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        ConnectivityPolicy::default(),
    );
    let error = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, ConnectivityError::Shared(ref source) if matches!(source.as_ref(), ConnectivityError::Task(_)))
    );
    assert!(!format!("{error:?} {error}").contains("private-panic-sentinel"));
    for _ in 0..2 {
        assert!(
            matches!(manager.shutdown().await, Err(ConnectivityError::Task(source)) if source.is_panic())
        );
    }
}

#[tokio::test]
async fn single_exchange_lease_cannot_open_a_second_stream() {
    let (manager, backend, _) = fixture(
        ConnectionMode::SingleExchange,
        ConnectivityPolicy::default(),
    );
    backend.gate.add_permits(1);
    let lease = request(&manager, 1).await.unwrap().unwrap();
    drop(
        lease
            .open_bi(Instant::now() + Duration::from_secs(1))
            .await
            .unwrap(),
    );
    assert!(matches!(
        lease
            .clone()
            .open_bi(Instant::now() + Duration::from_secs(1))
            .await,
        Err(ConnectivityError::ExchangeAlreadyUsed)
    ));
    drop(lease);
    manager.shutdown().await.unwrap();
}

/// 请求的旧撤销基线不能借用移除再添加之后建立的新活动连接。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_request_cannot_borrow_a_new_epoch_active_connection() {
    struct PausedBegin {
        epoch: Arc<AtomicUsize>,
        first: AtomicUsize,
        blocked: Arc<AtomicUsize>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    #[async_trait]
    impl SessionConnectionAdmissionPort for PausedBegin {
        fn begin(&self, peer: DeviceId) -> Box<dyn AdmissionAttempt> {
            let epoch = self.epoch.load(Ordering::SeqCst);
            if self.first.fetch_add(1, Ordering::SeqCst) == 0 {
                self.blocked.store(1, Ordering::SeqCst);
                self.release.lock().unwrap().recv().unwrap();
            }
            Box::new(Attempt {
                epoch,
                current: self.epoch.clone(),
                peer,
            })
        }
        async fn adopt_inbound(
            &self,
            _: Arc<dyn PeerConnection>,
            _reservation: &dyn InboundIdentityReservation,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            panic!("outbound-only fixture")
        }
    }
    let (_, backend, epoch) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let blocked = Arc::new(AtomicUsize::new(0));
    let (release, rx) = std::sync::mpsc::channel();
    let manager = ConnectivityManager::new(
        backend.clone(),
        Arc::new(PausedBegin {
            epoch: epoch.clone(),
            first: AtomicUsize::new(0),
            blocked: blocked.clone(),
            release: Mutex::new(rx),
        }),
        ConnectivityPolicy::default(),
    );
    let runtime = tokio::runtime::Handle::current();
    let old_manager = manager.clone();
    let old = std::thread::spawn(move || {
        runtime.block_on(old_manager.request(DeviceId::new("peer")).connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        ))
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while blocked.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    epoch.store(1, Ordering::SeqCst);
    backend.gate.add_permits(1);
    let new = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    release.send(()).unwrap();
    let old = tokio::task::spawn_blocking(move || old.join().unwrap())
        .await
        .unwrap();
    if let Ok(old) = old {
        assert!(
            old.open_bi(Instant::now() + Duration::from_secs(1))
                .await
                .is_err(),
            "the stale request used a new epoch's connection"
        );
    }
    assert!(new
        .open_bi(Instant::now() + Duration::from_secs(1))
        .await
        .is_ok());
    assert!(
        !backend.connections.lock().unwrap()[0].is_closed(),
        "rejecting the old request must not close the valid new connection"
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn completed_dials_racing_shutdown_cannot_escape_the_owner() {
    struct CompletingAdmission(Arc<tokio::sync::Notify>);
    struct CompletingAttempt(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl AdmissionAttempt for CompletingAttempt {
        async fn check_current(&self) -> Result<(), ConnectivityError> {
            Ok(())
        }
        async fn adopt(
            &self,
            _: Arc<dyn PeerConnection>,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            let grant = Grant {
                epoch: 0,
                current: Arc::new(AtomicUsize::new(0)),
                peer: DeviceId::new("peer"),
            };
            self.0.notify_one();
            Ok(Box::new(grant))
        }
    }
    #[async_trait]
    impl SessionConnectionAdmissionPort for CompletingAdmission {
        fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
            Box::new(CompletingAttempt(self.0.clone()))
        }
        async fn adopt_inbound(
            &self,
            _: Arc<dyn PeerConnection>,
            _reservation: &dyn InboundIdentityReservation,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            panic!("outbound-only fixture")
        }
    }
    for _ in 0..64 {
        let (_, backend, _) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
        let completed = Arc::new(tokio::sync::Notify::new());
        let manager = ConnectivityManager::new(
            backend.clone(),
            Arc::new(CompletingAdmission(completed.clone())),
            ConnectivityPolicy::default(),
        );
        let caller = request(&manager, 5);
        backend.gate.add_permits(1);
        completed.notified().await;
        manager.shutdown().await.unwrap();
        if let Ok(lease) = caller.await.unwrap() {
            assert!(
                lease
                    .open_bi(Instant::now() + Duration::from_secs(1))
                    .await
                    .is_err(),
                "a successful dial published after shutdown escaped closing"
            );
        }
        assert!(backend
            .connections
            .lock()
            .unwrap()
            .iter()
            .all(|connection| connection.is_closed()));
    }
}

#[tokio::test]
async fn queued_open_rechecks_its_request_epoch_after_capacity_is_available() {
    let (manager, backend, epoch) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            outbound_per_peer: 1,
            ..ConnectivityPolicy::default()
        },
    );
    backend.gate.add_permits(1);
    let lease = request(&manager, 5).await.unwrap().unwrap();
    let first = lease
        .open_bi(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    let pending =
        tokio::spawn(async move { lease.open_bi(Instant::now() + Duration::from_secs(1)).await });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    epoch.fetch_add(1, Ordering::SeqCst);
    drop(first);
    assert!(matches!(
        pending.await.unwrap(),
        Err(ConnectivityError::Denied)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_adoption_closes_a_connection_retained_by_the_backend() {
    struct BlockingAdmission(Arc<tokio::sync::Notify>);
    struct BlockingAttempt(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl AdmissionAttempt for BlockingAttempt {
        async fn check_current(&self) -> Result<(), ConnectivityError> {
            Ok(())
        }
        async fn adopt(
            &self,
            _: Arc<dyn PeerConnection>,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            self.0.notify_one();
            std::future::pending().await
        }
    }
    #[async_trait]
    impl SessionConnectionAdmissionPort for BlockingAdmission {
        fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
            Box::new(BlockingAttempt(self.0.clone()))
        }
        async fn adopt_inbound(
            &self,
            _: Arc<dyn PeerConnection>,
            _reservation: &dyn InboundIdentityReservation,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            panic!("outbound-only fixture")
        }
    }
    let (_, backend, _) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let adopting = Arc::new(tokio::sync::Notify::new());
    let manager = ConnectivityManager::new(
        backend.clone(),
        Arc::new(BlockingAdmission(adopting.clone())),
        ConnectivityPolicy::default(),
    );
    backend.gate.add_permits(1);
    let caller = request(&manager, 5);
    adopting.notified().await;
    caller.abort();
    let _ = caller.await;
    manager.shutdown().await.unwrap();
    assert!(backend.connections.lock().unwrap()[0].is_closed());
}

#[tokio::test(start_paused = true)]
async fn idle_retention_reuses_across_requests_and_expires_after_last_activity() {
    let (manager, backend, _) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            idle_retention: Some(super::IdleRetentionPolicy {
                elapsed_clock: None,
                timeout: Duration::from_secs(30),
                capacity: 16,
            }),
            ..ConnectivityPolicy::default()
        },
    );
    backend.gate.add_permits(2);
    let target = DeviceId::new("history-peer");
    let first = manager
        .request(target)
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    manager.retain(&first).await.unwrap();
    drop(first);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    let second = manager
        .request(target)
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(
        !backend.connections.lock().unwrap()[0].is_closed(),
        "active request must survive idle timer"
    );
    drop(second);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_first_frame_cannot_adopt_a_grant_from_after_revocation() {
    let (manager, _, epoch) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let (closed, _) = watch::channel(false);
    let raw = Arc::new(TestConnection {
        id: 99,
        mode: ConnectionMode::Multiplexed,
        closed,
    });
    let pending = manager
        .accept_pending(raw.clone(), Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    epoch.store(1, Ordering::SeqCst);
    let result = pending
        .bind(Box::new(Grant {
            epoch: 1,
            current: epoch,
            peer: DeviceId::new("introduced"),
        }))
        .await;
    assert!(matches!(result, Err(ConnectivityError::Denied)));
    assert!(raw.is_closed());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn retention_capacity_does_not_evict_active_requests() {
    let (manager, backend, _) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            idle_retention: Some(super::IdleRetentionPolicy {
                elapsed_clock: None,
                timeout: Duration::from_secs(30),
                capacity: 1,
            }),
            ..ConnectivityPolicy::default()
        },
    );
    backend.gate.add_permits(3);
    let first = manager
        .request(DeviceId::new("a"))
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&first).await.unwrap());
    let second = manager
        .request(DeviceId::new("b"))
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!manager.retain(&second).await.unwrap());
    drop(second);
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    assert!(backend.connections.lock().unwrap()[1].is_closed());
    drop(first);
    let third = manager
        .request(DeviceId::new("c"))
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&third).await.unwrap());
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}

struct CountingAccept {
    raw: TestConnection,
    accepted: AtomicUsize,
}
#[async_trait]
impl PeerConnection for CountingAccept {
    fn id(&self) -> usize {
        self.raw.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [0; 32]
    }
    fn mode(&self) -> ConnectionMode {
        ConnectionMode::Multiplexed
    }
    fn protocol(&self) -> ConnectionProtocol {
        ConnectionProtocol::MultiplexedMembershipHistory
    }
    fn channel(&self) -> ConnectionChannel {
        ConnectionChannel::Unknown
    }
    fn is_closed(&self) -> bool {
        self.raw.is_closed()
    }
    fn close(&self) {
        self.raw.close();
    }
    async fn closed(&self) {
        self.raw.closed().await;
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.raw.open_bi().await
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.accepted.fetch_add(1, Ordering::SeqCst);
        self.raw.accept_bi().await
    }
}

#[tokio::test]
async fn cancelled_accept_while_waiting_for_capacity_keeps_the_already_received_stream() {
    let (manager, _, epoch) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            inbound_per_session: 1,
            ..ConnectivityPolicy::default()
        },
    );
    let (closed, _) = watch::channel(false);
    let raw = Arc::new(CountingAccept {
        raw: TestConnection {
            id: 123,
            mode: ConnectionMode::Multiplexed,
            closed,
        },
        accepted: AtomicUsize::new(0),
    });
    let pending = manager
        .accept_pending(raw.clone(), Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    let (connection, first) = pending
        .bind(Box::new(Grant {
            epoch: 0,
            current: epoch,
            peer: DeviceId::new("peer"),
        }))
        .await
        .unwrap();
    drop(first);
    let blocker = connection.accept_bi().await.unwrap();
    let mut waiting = Box::pin(connection.accept_bi());
    assert!(futures::poll!(&mut waiting).is_pending());
    assert_eq!(raw.accepted.load(Ordering::SeqCst), 3);
    drop(waiting);
    drop(blocker);
    let received = connection.accept_bi().await.unwrap();
    assert_eq!(
        raw.accepted.load(Ordering::SeqCst),
        3,
        "the received request must survive select cancellation"
    );
    drop(received);
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn refreshing_dead_transport_preserves_the_original_request_revocation_fence() {
    let (manager, backend, epoch) =
        fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    backend.gate.add_permits(2);
    let lease = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    lease.close();
    epoch.store(1, Ordering::SeqCst);
    assert!(matches!(
        manager
            .refresh(
                lease,
                Instant::now() + Duration::from_secs(10),
                &CancellationToken::new()
            )
            .await,
        Err(ConnectivityError::Denied)
    ));
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "a revoked old request cannot dial under a new grant"
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn evicting_reuse_keeps_a_brother_stream_until_its_own_completion() {
    let (manager, backend, _) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            idle_retention: Some(super::IdleRetentionPolicy {
                elapsed_clock: None,
                timeout: Duration::from_secs(30),
                capacity: 16,
            }),
            ..ConnectivityPolicy::default()
        },
    );
    backend.gate.add_permits(2);
    let lease = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&lease).await.unwrap());
    let brother = lease
        .open_bi(Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    manager.evict(&lease);
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    let replacement = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(10),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert!(
        !backend.connections.lock().unwrap()[0].is_closed(),
        "eviction must not cancel an active brother stream"
    );
    drop(lease);
    drop(brother);
    tokio::task::yield_now().await;
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    drop(replacement);
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn inbound_queue_wait_does_not_restart_the_exchange_budget() {
    let (manager, _, epoch) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            inbound_per_session: 1,
            accepted_stream_timeout: Some(Duration::from_secs(10)),
            ..ConnectivityPolicy::default()
        },
    );
    let (closed, _) = watch::channel(false);
    let raw = Arc::new(TestConnection {
        id: 321,
        mode: ConnectionMode::Multiplexed,
        closed,
    });
    let pending = manager
        .accept_pending(raw, Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    let (connection, first) = pending
        .bind(Box::new(Grant {
            epoch: 0,
            current: epoch,
            peer: DeviceId::new("peer"),
        }))
        .await
        .unwrap();
    drop(first);
    let blocker = connection.accept_bi().await.unwrap();
    let started = Instant::now();
    let mut waiting = Box::pin(connection.accept_bi());
    assert!(futures::poll!(&mut waiting).is_pending());
    tokio::time::advance(Duration::from_secs(9)).await;
    drop(blocker);
    let received = waiting.await.unwrap();
    let deadline = received
        .received_at
        .expect("stream carries its original receive time")
        + Duration::from_secs(10);
    assert!(
        tokio::time::timeout_at(deadline, std::future::pending::<()>())
            .await
            .is_err()
    );
    assert_eq!(Instant::now() - started, Duration::from_secs(10));
    drop(received);
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn expired_inbound_queue_discards_only_its_stream_and_returns_capacity() {
    let (manager, _, epoch) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            inbound_per_session: 1,
            accepted_stream_timeout: Some(Duration::from_secs(10)),
            ..ConnectivityPolicy::default()
        },
    );
    let (closed, _) = watch::channel(false);
    let raw = Arc::new(CountingAccept {
        raw: TestConnection {
            id: 654,
            mode: ConnectionMode::Multiplexed,
            closed,
        },
        accepted: AtomicUsize::new(0),
    });
    let pending = manager
        .accept_pending(raw.clone(), Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    let (connection, first) = pending
        .bind(Box::new(Grant {
            epoch: 0,
            current: epoch,
            peer: DeviceId::new("peer"),
        }))
        .await
        .unwrap();
    drop(first);
    let mut brother = connection.accept_bi().await.unwrap();
    let mut waiting = Box::pin(connection.accept_bi());
    assert!(futures::poll!(&mut waiting).is_pending());
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(waiting.await, Err(ConnectivityError::TimedOut(_))));
    assert!(!raw.is_closed());
    brother.send.finish().unwrap();
    drop(brother);
    let received = connection.accept_bi().await.unwrap();
    assert_eq!(
        raw.accepted.load(Ordering::SeqCst),
        4,
        "expired queued frame must not remain cached"
    );
    drop(received);
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_work_keeps_exchange_capacity_after_network_stream_is_dropped() {
    for split in [false, true] {
        let (manager, _, epoch) = fixture(
            ConnectionMode::Multiplexed,
            ConnectivityPolicy {
                inbound_per_session: 1,
                ..ConnectivityPolicy::default()
            },
        );
        let (closed, _) = watch::channel(false);
        let raw = Arc::new(TestConnection {
            id: 987,
            mode: ConnectionMode::Multiplexed,
            closed,
        });
        let pending = manager
            .accept_pending(raw, Instant::now() + Duration::from_secs(10))
            .await
            .unwrap();
        let (connection, stream) = pending
            .bind(Box::new(Grant {
                epoch: 0,
                current: epoch,
                peer: DeviceId::new("peer"),
            }))
            .await
            .unwrap();
        let work = if split {
            let (parts, work) = stream.into_parts();
            drop(parts);
            work
        } else {
            let work = stream.hold_for_work();
            drop(stream);
            work
        };
        let mut following = Box::pin(connection.accept_bi());
        assert!(
            futures::poll!(&mut following).is_pending(),
            "started local work still owns the only exchange slot"
        );
        drop(work);
        let following = following.await.unwrap();
        drop(following);
        manager.shutdown().await.unwrap();
    }
}

/// 单首轮能力与保留生命周期分开：FIN 后租约继续存活，不能再开第二次首轮流。
#[tokio::test]
async fn retained_single_exchange_survives_fin_until_its_owner_releases_it() {
    let (manager, backend, _) = fixture(
        ConnectionMode::RetainedSingleExchange,
        ConnectivityPolicy::default(),
    );
    backend.gate.add_permits(1);
    let lease = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut stream = lease
        .open_bi(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    stream.send.finish().unwrap();
    drop(stream);
    assert!(!lease.is_closed());
    assert!(matches!(
        lease.open_bi(Instant::now() + Duration::from_secs(1)).await,
        Err(ConnectivityError::ExchangeAlreadyUsed)
    ));
    drop(lease);
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}

/// pending 的每端许可必须持有同一 PeerState；长期连接不能占据固定入口总額。
#[tokio::test]
async fn presence_pending_capacity_is_shared_per_peer_and_released_after_promotion() {
    let (manager, _, epoch) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            presence_retention: true,
            ..ConnectivityPolicy::default()
        },
    );
    let raw = |id| {
        let (closed, _) = watch::channel(false);
        Arc::new(TestConnection {
            id,
            mode: ConnectionMode::Multiplexed,
            closed,
        }) as Arc<dyn PeerConnection>
    };
    let peer = DeviceId::new("peer");
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut first = manager.accept_pending(raw(1), deadline).await.unwrap();
    first.reserve_peer(peer).unwrap();
    let mut second = manager.accept_pending(raw(2), deadline).await.unwrap();
    second.reserve_peer(peer).unwrap();
    let mut third = manager.accept_pending(raw(3), deadline).await.unwrap();
    assert!(matches!(
        third.reserve_peer(peer),
        Err(ConnectivityError::Busy(_))
    ));
    let grant = Box::new(Grant {
        epoch: 0,
        current: epoch,
        peer,
    });
    let (long, stream) = first.bind(grant).await.unwrap();
    drop(stream);
    third.reserve_peer(peer).unwrap();
    assert!(!long.is_closed());
    drop((second, third, long));
    manager.shutdown().await.unwrap();
}

/// 逻辑超时不能取消已经开始的本地读取并提前归还活动许可。
#[tokio::test(start_paused = true)]
async fn presence_stream_deadline_waits_for_started_local_read_to_settle() {
    struct SlowGrant {
        peer: DeviceId,
        gate: Arc<Semaphore>,
        started: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl AdmissionGrant for SlowGrant {
        fn peer(&self) -> DeviceId {
            self.peer
        }
        async fn can_retain(&self) -> Result<bool, ConnectivityError> {
            Ok(true)
        }
        async fn check_current(&self) -> Result<(), ConnectivityError> {
            if self.started.fetch_add(1, Ordering::SeqCst) > 0 {
                self.gate.acquire().await.unwrap().forget();
            }
            Ok(())
        }
    }
    let (manager, _, _) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            presence_retention: true,
            ..ConnectivityPolicy::default()
        },
    );
    let (closed, _) = watch::channel(false);
    let raw = Arc::new(TestConnection {
        id: 1,
        mode: ConnectionMode::Multiplexed,
        closed,
    });
    let peer = DeviceId::new("peer");
    let mut pending = manager
        .accept_pending(raw, Instant::now() + Duration::from_secs(20))
        .await
        .unwrap();
    pending.reserve_peer(peer).unwrap();
    let gate = Arc::new(Semaphore::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let (lease, first) = pending
        .bind(Box::new(SlowGrant {
            peer,
            gate: gate.clone(),
            started: started.clone(),
        }))
        .await
        .unwrap();
    drop(first);
    let job =
        tokio::spawn(async move { lease.open_bi(Instant::now() + Duration::from_secs(1)).await });
    while started.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert!(
        !job.is_finished(),
        "started local work must retain its activity until actual settlement"
    );
    gate.add_permits(1);
    assert!(matches!(
        job.await.unwrap(),
        Err(ConnectivityError::AdmissionTimedOut(_))
    ));
    manager.shutdown().await.unwrap();
}

/// 取消停止后续握手；已经开始的第二 backend 本地工作仍占唯一 flight 许可。
#[tokio::test]
async fn presence_cancelled_acquisition_waits_for_real_work_before_returning_flight_capacity() {
    let (manager, backend, _) = fixture(
        ConnectionMode::Multiplexed,
        ConnectivityPolicy {
            presence_retention: true,
            flights_per_session: 1,
            ..ConnectivityPolicy::default()
        },
    );
    let cancel = CancellationToken::new();
    let job = tokio::spawn({
        let manager = manager.clone();
        let cancel = cancel.clone();
        async move {
            manager
                .request(DeviceId::new("one"))
                .connect(Instant::now() + Duration::from_secs(20), &cancel)
                .await
        }
    });
    while backend.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    cancel.cancel();
    tokio::task::yield_now().await;
    assert!(!job.is_finished());
    assert!(matches!(
        manager
            .request(DeviceId::new("two"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new()
            )
            .await,
        Err(ConnectivityError::Busy(_))
    ));
    backend.gate.add_permits(1);
    assert!(matches!(
        job.await.unwrap(),
        Err(ConnectivityError::Stopped)
    ));
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    backend.gate.add_permits(1);
    let lease = manager
        .request(DeviceId::new("two"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    drop(lease);
    manager.shutdown().await.unwrap();
}

#[path = "inbound_tests.rs"]
mod inbound;

#[path = "lifetime_tests.rs"]
mod lifetime;

#[tokio::test]
async fn identity_lookup_scope_cannot_gain_a_fresh_grant_after_revocation_or_session_restart() {
    struct Fence {
        epoch: usize,
        current: Arc<AtomicUsize>,
    }
    impl super::InboundAdmissionFence for Fence {
        fn check_current(&self, _: &DeviceId) -> Result<(), ConnectivityError> {
            if self.current.load(Ordering::SeqCst) == self.epoch {
                Ok(())
            } else {
                Err(ConnectivityError::Denied)
            }
        }
    }
    struct ScopedAdmission(Arc<AtomicUsize>);
    #[async_trait]
    impl SessionConnectionAdmissionPort for ScopedAdmission {
        fn inbound_fence(&self) -> Option<Box<dyn super::InboundAdmissionFence>> {
            Some(Box::new(Fence {
                epoch: self.0.load(Ordering::SeqCst),
                current: self.0.clone(),
            }))
        }
        fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
            Admission(self.0.clone()).begin(target)
        }
        async fn adopt_inbound(
            &self,
            _: Arc<dyn PeerConnection>,
            _: &dyn InboundIdentityReservation,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            panic!("outbound scope fixture")
        }
    }
    let (_, backend, epoch) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let manager = ConnectivityManager::new(
        backend.clone(),
        Arc::new(ScopedAdmission(epoch.clone())),
        ConnectivityPolicy::default(),
    );
    let before_identity = manager.freeze_request();
    epoch.store(1, Ordering::SeqCst);
    assert!(matches!(
        before_identity.bind(DeviceId::new("peer"), Vec::new()),
        Err(ConnectivityError::Denied)
    ));
    let before_queue = manager
        .freeze_request()
        .bind(DeviceId::new("peer"), Vec::new())
        .expect("current request");
    epoch.store(2, Ordering::SeqCst);
    assert!(matches!(
        before_queue
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new()
            )
            .await,
        Err(ConnectivityError::Denied)
    ));
    let old_session = manager.freeze_request();
    manager.disconnect_all().await.expect("restart session");
    assert!(matches!(
        old_session.bind(DeviceId::new("peer"), Vec::new()),
        Err(ConnectivityError::Stopped)
    ));
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        0,
        "old request must never dial under renewed authority"
    );
    manager.shutdown().await.expect("shutdown");
}

#[path = "retention_tests.rs"]
mod retention_tests;
