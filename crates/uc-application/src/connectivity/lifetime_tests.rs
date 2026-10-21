//! 关闭动作与实际关闭完成分开，证明名额不会仅因 record 或网络流释放而归还。
use super::{Admission, TestConnection};
use crate::connectivity::{
    ConnectionMode, ConnectionProtocol, ConnectivityBackend, ConnectivityError,
    ConnectivityManager, ConnectivityPolicy, PeerConnection, StreamPair,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;

pub(super) struct ClosingConnection {
    inner: TestConnection,
    pub(super) finished: watch::Sender<bool>,
}
impl ClosingConnection {
    pub(super) fn new(id: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: TestConnection {
                id,
                mode: ConnectionMode::Multiplexed,
                closed: watch::channel(false).0,
            },
            finished: watch::channel(false).0,
        })
    }
}
#[async_trait]
impl PeerConnection for ClosingConnection {
    fn id(&self) -> usize {
        self.inner.id()
    }
    fn authenticated_key(&self) -> [u8; 32] {
        self.inner.authenticated_key()
    }
    fn mode(&self) -> ConnectionMode {
        self.inner.mode()
    }
    fn protocol(&self) -> ConnectionProtocol {
        self.inner.protocol()
    }
    fn channel(&self) -> ConnectionChannel {
        self.inner.channel()
    }
    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
    fn close(&self) {
        self.inner.close();
    }
    async fn closed(&self) {
        let mut finished = self.finished.subscribe();
        while !*finished.borrow_and_update() {
            finished.changed().await.expect("close gate");
        }
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.inner.open_bi().await
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.inner.accept_bi().await
    }
}
struct Backend {
    entered: Semaphore,
    finish: Semaphore,
    calls: AtomicUsize,
    connections: Mutex<Vec<Arc<ClosingConnection>>>,
}
#[async_trait]
impl ConnectivityBackend for Backend {
    async fn connect(
        &self,
        _: DeviceId,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let id = self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.finish.acquire().await.expect("dial finish").forget();
        let connection = ClosingConnection::new(id);
        self.connections
            .lock()
            .expect("connections")
            .push(connection.clone());
        Ok(connection)
    }
}
fn setup() -> (ConnectivityManager, Arc<Backend>) {
    let backend = Arc::new(Backend {
        entered: Semaphore::new(0),
        finish: Semaphore::new(0),
        calls: AtomicUsize::new(0),
        connections: Mutex::new(Vec::new()),
    });
    let manager = ConnectivityManager::new(
        backend.clone(),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        ConnectivityPolicy {
            connections_per_peer_direction: Some(1),
            ..ConnectivityPolicy::default()
        },
    );
    (manager, backend)
}
async fn busy(manager: &ConnectivityManager) {
    let error = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .err()
        .expect("second connection accepted");
    assert_eq!(error.class(), "connection_busy");
}
async fn settle_watchers() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn outgoing_direction_slot_waits_for_both_actual_close_and_local_work_in_either_order() {
    for close_first in [false, true] {
        let (manager, backend) = setup();
        backend.finish.add_permits(1);
        let lease = manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new(),
            )
            .await
            .expect("first connection");
        let stream = lease
            .open_bi(Instant::now() + Duration::from_secs(1))
            .await
            .expect("stream");
        let (io, work) = stream.into_parts();
        drop(io);
        let first = backend.connections.lock().expect("connections")[0].clone();
        lease.close();
        drop(lease);
        if close_first {
            first.finished.send_replace(true);
            settle_watchers().await;
            busy(&manager).await;
            drop(work);
        } else {
            drop(work);
            settle_watchers().await;
            busy(&manager).await;
            first.finished.send_replace(true);
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        settle_watchers().await;
        backend.finish.add_permits(1);
        let next = manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new(),
            )
            .await
            .expect("slot returned after both completions");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
        backend.connections.lock().expect("connections")[1]
            .finished
            .send_replace(true);
        drop(next);
        manager.shutdown().await.expect("shutdown");
    }
}

#[tokio::test(start_paused = true)]
async fn outgoing_direction_slot_also_covers_cancelled_but_unsettled_dial_and_its_late_close() {
    let (manager, backend) = setup();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let owner = manager.clone();
    let first = tokio::spawn(async move {
        owner
            .request(DeviceId::new("peer"))
            .connect(Instant::now() + Duration::from_secs(10), &token)
            .await
    });
    timeout(Duration::from_secs(1), backend.entered.acquire())
        .await
        .expect("dial started")
        .expect("dial signal")
        .forget();
    cancel.cancel();
    settle_watchers().await;
    busy(&manager).await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    backend.finish.add_permits(1);
    assert!(first.await.expect("first task").is_err());
    let connection = backend.connections.lock().expect("connections")[0].clone();
    assert!(connection.is_closed());
    busy(&manager).await;
    connection.finished.send_replace(true);
    settle_watchers().await;
    backend.finish.add_permits(1);
    let next = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .expect("settled direction available");
    backend.connections.lock().expect("connections")[1]
        .finished
        .send_replace(true);
    drop(next);
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn shutdown_joins_close_work_registered_by_a_late_returning_dial() {
    let (manager, backend) = setup();
    let owner = manager.clone();
    let waiting = tokio::spawn(async move {
        owner
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(10),
                &CancellationToken::new(),
            )
            .await
    });
    timeout(Duration::from_secs(1), backend.entered.acquire())
        .await
        .expect("dial started")
        .expect("dial signal")
        .forget();
    let owner = manager.clone();
    let closing = tokio::spawn(async move { owner.shutdown().await });
    settle_watchers().await;
    assert!(!closing.is_finished());
    backend.finish.add_permits(1);
    assert!(waiting.await.expect("waiter").is_err());
    settle_watchers().await;
    assert!(
        !closing.is_finished(),
        "late connection close escaped shutdown collection"
    );
    let connection = backend.connections.lock().expect("connections")[0].clone();
    assert!(connection.is_closed());
    connection.finished.send_replace(true);
    closing.await.expect("shutdown task").expect("shutdown");
}
