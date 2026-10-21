use super::super::{
    BlobExchangeError, BlobExchangeFailure, BlobExchangePort, BlobReceived, BlobStoreLifecyclePort,
    BlobValidatedProgress,
};
use super::{BlobShutdownWait, BlobTransport};
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectionProtocol, ConnectivityBackend,
    ConnectivityError, InboundIdentityReservation, PeerConnection, PeerReceiveStream,
    PeerSendStream, SessionConnectionAdmissionPort, StreamLease, StreamPair,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::io;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, watch, Mutex, Semaphore};
use tokio::time::{advance, timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::blob::BlobDigest;
use uc_core::ports::ConnectionChannel;

struct Backend;
#[async_trait]
impl ConnectivityBackend for Backend {
    async fn connect(
        &self,
        _: DeviceId,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        panic!("provider must not dial");
    }
}
struct Grant(DeviceId);
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.0
    }
    async fn can_retain(&self) -> Result<bool, ConnectivityError> {
        Ok(true)
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
}
struct Admission {
    entered: Semaphore,
    finish: Semaphore,
    calls: AtomicUsize,
}
#[async_trait]
impl SessionConnectionAdmissionPort for Admission {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        panic!("outbound admission");
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let peer = DeviceId::new(format!("peer-{}", connection.authenticated_key()[0]));
        reservation.identified(peer)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.finish
            .acquire()
            .await
            .expect("admission gate")
            .forget();
        Ok(Box::new(Grant(peer)))
    }
}
struct SendHalf(Arc<Semaphore>);
impl AsyncWrite for SendHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl PeerSendStream for SendHalf {
    fn id(&self) -> u64 {
        0
    }
    fn reset(&mut self, _: u64) -> Result<(), ConnectivityError> {
        self.0.add_permits(1);
        Ok(())
    }
    fn finish(&mut self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>> {
        Box::pin(async { Ok(()) })
    }
}
struct ReceiveHalf(Arc<Semaphore>);
impl AsyncRead for ReceiveHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl PeerReceiveStream for ReceiveHalf {
    fn id(&self) -> u64 {
        0
    }
    fn stop(&mut self, _: u64) -> Result<(), ConnectivityError> {
        self.0.add_permits(1);
        Ok(())
    }
}
struct Connection {
    id: usize,
    peer: u8,
    streams: Mutex<mpsc::Receiver<StreamPair>>,
    closed: watch::Sender<bool>,
}
#[async_trait]
impl PeerConnection for Connection {
    fn id(&self) -> usize {
        self.id
    }
    fn authenticated_key(&self) -> [u8; 32] {
        [self.peer; 32]
    }
    fn mode(&self) -> ConnectionMode {
        ConnectionMode::Multiplexed
    }
    fn protocol(&self) -> ConnectionProtocol {
        ConnectionProtocol::Blob
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
        let mut closed = self.closed.subscribe();
        while !*closed.borrow_and_update() {
            closed.changed().await.expect("closed signal");
        }
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        panic!("provider must not open a stream");
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        let mut streams = self.streams.lock().await;
        tokio::select! {
            biased;
            _ = self.closed() => Err(ConnectivityError::Stream(Box::new(io::Error::from(io::ErrorKind::ConnectionReset)))),
            stream = streams.recv() => stream.ok_or(ConnectivityError::Stopped),
        }
    }
}
struct Exchange {
    entered: Semaphore,
    cancelled: Semaphore,
    finish: Semaphore,
    active: AtomicUsize,
    deadlines: StdMutex<Vec<Instant>>,
    closed: AtomicUsize,
    fail_export: AtomicBool,
    fail_close: AtomicBool,
    panic_export: AtomicBool,
    immediate: AtomicBool,
}
#[async_trait]
impl BlobExchangePort for Exchange {
    async fn receive(
        &self,
        _: StreamLease,
        _: BlobDigest,
        _: CancellationToken,
        _: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        panic!("provider receive");
    }
    async fn provide(
        &self,
        stream: StreamLease,
        deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        self.deadlines.lock().expect("deadlines").push(deadline);
        struct Active<'a>(&'a AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _active = Active(&self.active);
        self.entered.add_permits(1);
        assert!(
            !self.panic_export.load(Ordering::SeqCst),
            "synthetic provider task panic"
        );
        if self.immediate.load(Ordering::SeqCst) {
            drop(stream);
            return Ok(());
        }
        cancel.cancelled().await;
        self.cancelled.add_permits(1);
        self.finish
            .acquire()
            .await
            .expect("actual export finish")
            .forget();
        drop(stream);
        if self.fail_export.load(Ordering::SeqCst) {
            return Err(BlobExchangeError::new(
                BlobExchangeFailure::Store,
                io::Error::from(io::ErrorKind::PermissionDenied),
            ));
        }
        Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled))
    }
}
#[async_trait]
impl BlobStoreLifecyclePort for Exchange {
    async fn shutdown(&self) -> Result<(), BlobExchangeError> {
        assert_eq!(
            self.active.load(Ordering::SeqCst),
            0,
            "store closed before exports settled"
        );
        self.closed.fetch_add(1, Ordering::SeqCst);
        if self.fail_close.load(Ordering::SeqCst) {
            return Err(BlobExchangeError::new(
                BlobExchangeFailure::Store,
                io::Error::from(io::ErrorKind::BrokenPipe),
            ));
        }
        Ok(())
    }
}
fn setup(admit: usize) -> (Arc<BlobTransport>, Arc<Admission>, Arc<Exchange>) {
    let admission = Arc::new(Admission {
        entered: Semaphore::new(0),
        finish: Semaphore::new(admit),
        calls: AtomicUsize::new(0),
    });
    let exchange = Arc::new(Exchange {
        entered: Semaphore::new(0),
        cancelled: Semaphore::new(0),
        finish: Semaphore::new(0),
        active: AtomicUsize::new(0),
        deadlines: StdMutex::new(Vec::new()),
        closed: AtomicUsize::new(0),
        fail_export: AtomicBool::new(false),
        fail_close: AtomicBool::new(false),
        panic_export: AtomicBool::new(false),
        immediate: AtomicBool::new(false),
    });
    (
        BlobTransport::new(
            Arc::new(Backend),
            admission.clone(),
            exchange.clone(),
            exchange.clone(),
        ),
        admission,
        exchange,
    )
}
fn connection(id: usize, peer: u8) -> (Arc<Connection>, mpsc::Sender<StreamPair>) {
    let (send, streams) = mpsc::channel(8);
    (
        Arc::new(Connection {
            id,
            peer,
            streams: Mutex::new(streams),
            closed: watch::channel(false).0,
        }),
        send,
    )
}
async fn stream(send: &mpsc::Sender<StreamPair>, stopped: &Arc<Semaphore>) {
    send.send((
        Box::new(SendHalf(stopped.clone())),
        Box::new(ReceiveHalf(stopped.clone())),
    ))
    .await
    .map_err(|_| ())
    .expect("stream queue");
}
async fn entered(signal: &Semaphore, count: u32) {
    timeout(Duration::from_secs(1), signal.acquire_many(count))
        .await
        .expect("work not entered")
        .expect("signal")
        .forget();
}

#[tokio::test(start_paused = true)]
async fn provider_rejects_excess_per_peer_and_global_streams_without_closing_siblings_or_queuing_sdk_work(
) {
    let (owner, _, exchange) = setup(3);
    let stopped = Arc::new(Semaphore::new(0));
    let (first, first_send) = connection(1, 1);
    let (second, second_send) = connection(2, 2);
    let (third, third_send) = connection(3, 3);
    let first_job = tokio::spawn(owner.serve(first.clone()));
    let second_job = tokio::spawn(owner.serve(second.clone()));
    let third_job = tokio::spawn(owner.serve(third.clone()));
    for _ in 0..2 {
        stream(&first_send, &stopped).await;
        stream(&second_send, &stopped).await;
    }
    entered(&exchange.entered, 4).await;
    stream(&first_send, &stopped).await;
    stream(&third_send, &stopped).await;
    entered(&stopped, 4).await;
    assert_eq!(exchange.active.load(Ordering::SeqCst), 4);
    assert_eq!(
        exchange.entered.available_permits(),
        0,
        "excess request entered SDK"
    );
    assert!(!first.is_closed() && !second.is_closed() && !third.is_closed());
    let closing = owner.shutdown(BlobShutdownWait::Complete);
    entered(&exchange.cancelled, 4).await;
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 0);
    assert_eq!(exchange.active.load(Ordering::SeqCst), 4);
    exchange.finish.add_permits(4);
    closing.await.expect("complete close");
    first_job.await.expect("first task").expect("first service");
    second_job
        .await
        .expect("second task")
        .expect("second service");
    third_job.await.expect("third task").expect("third service");
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn provider_header_uses_admission_start_and_abandoned_close_stays_supervised_after_five_seconds(
) {
    let (owner, admission, exchange) = setup(0);
    let stopped = Arc::new(Semaphore::new(0));
    let (connection, send) = connection(1, 1);
    let started = Instant::now();
    let service = tokio::spawn(owner.serve(connection));
    stream(&send, &stopped).await;
    entered(&admission.entered, 1).await;
    advance(Duration::from_secs(50)).await;
    admission.finish.add_permits(1);
    entered(&exchange.entered, 1).await;
    assert_eq!(
        exchange.deadlines.lock().expect("deadline")[0],
        started + Duration::from_secs(60)
    );
    service.abort();
    assert!(service
        .await
        .expect_err("service waiter survived")
        .is_cancelled());
    entered(&exchange.cancelled, 1).await;
    drop(owner.shutdown(BlobShutdownWait::Bounded));
    let waiting = tokio::spawn(owner.shutdown(BlobShutdownWait::Bounded));
    tokio::task::yield_now().await;
    advance(Duration::from_secs(5)).await;
    let error = waiting
        .await
        .expect("close wait")
        .expect_err("slow work reported closed");
    assert_eq!(error.kind, BlobExchangeFailure::ShutdownIncomplete);
    assert_eq!(exchange.active.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 0);
    exchange.finish.add_permits(1);
    owner
        .shutdown(BlobShutdownWait::Complete)
        .await
        .expect("actual close");
    owner
        .shutdown(BlobShutdownWait::Bounded)
        .await
        .expect("same completed close");
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.deadlines.lock().expect("attempt count").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn empty_warmup_does_not_read_members_or_take_the_authorized_peer_slot() {
    let (owner, admission, exchange) = setup(1);
    let stopped = Arc::new(Semaphore::new(0));
    let (warmup, _warmup_send) = connection(1, 1);
    let warmup_job = tokio::spawn(owner.serve(warmup.clone()));
    tokio::task::yield_now().await;
    assert_eq!(admission.calls.load(Ordering::SeqCst), 0);
    let (data, send) = connection(2, 1);
    let data_job = tokio::spawn(owner.serve(data));
    stream(&send, &stopped).await;
    entered(&exchange.entered, 1).await;
    assert_eq!(admission.calls.load(Ordering::SeqCst), 1);
    assert!(!warmup.is_closed());
    let closing = owner.shutdown(BlobShutdownWait::Complete);
    entered(&exchange.cancelled, 1).await;
    exchange.finish.add_permits(1);
    closing.await.expect("close");
    assert!(warmup_job.await.expect("warmup task").is_err());
    data_job.await.expect("data task").expect("data service");
}

#[tokio::test(start_paused = true)]
async fn provider_preserves_failed_export_and_store_close_sources_for_repeated_shutdown() {
    let (owner, _, exchange) = setup(1);
    exchange.fail_export.store(true, Ordering::SeqCst);
    exchange.fail_close.store(true, Ordering::SeqCst);
    let stopped = Arc::new(Semaphore::new(0));
    let (connection, send) = connection(1, 1);
    let service = tokio::spawn(owner.serve(connection));
    stream(&send, &stopped).await;
    entered(&exchange.entered, 1).await;
    let closing = owner.shutdown(BlobShutdownWait::Complete);
    entered(&exchange.cancelled, 1).await;
    exchange.finish.add_permits(1);
    let error = closing.await.expect_err("failed cleanup reported success");
    assert_eq!(error.kind, BlobExchangeFailure::Settlement);
    let report = error
        .source
        .downcast_ref::<Arc<BlobExchangeError>>()
        .expect("shared complete shutdown report");
    assert_eq!(
        report
            .source
            .chain()
            .find_map(|source| source.downcast_ref::<io::Error>())
            .expect("original export error")
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(report.additional.len(), 1);
    assert_eq!(
        report.additional[0]
            .source
            .chain()
            .find_map(|source| source.downcast_ref::<io::Error>())
            .expect("original close error")
            .kind(),
        io::ErrorKind::BrokenPipe
    );
    assert!(service.await.expect("service task").is_err());
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 1);
    assert_eq!(
        owner
            .shutdown(BlobShutdownWait::Complete)
            .await
            .expect_err("repeat close forgot failure")
            .kind,
        BlobExchangeFailure::Settlement
    );
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn provider_task_panic_ends_the_listener_immediately_and_is_retained_for_close() {
    let (owner, _, exchange) = setup(1);
    exchange.panic_export.store(true, Ordering::SeqCst);
    let stopped = Arc::new(Semaphore::new(0));
    let (connection, send) = connection(1, 1);
    let service = tokio::spawn(owner.serve(connection.clone()));
    stream(&send, &stopped).await;
    let error = timeout(Duration::from_secs(1), service)
        .await
        .expect("panic waited for idle timeout")
        .expect("service waiter")
        .expect_err("panic succeeded");
    assert_eq!(error.kind, BlobExchangeFailure::Task);
    assert!(error
        .source
        .chain()
        .find_map(|source| source.downcast_ref::<tokio::task::JoinError>())
        .expect("original join failure")
        .is_panic());
    assert!(connection.is_closed());
    assert_eq!(exchange.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        owner
            .shutdown(BlobShutdownWait::Complete)
            .await
            .expect_err("close forgot worker panic")
            .kind,
        BlobExchangeFailure::Task
    );
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_new_peer_can_evict_an_idle_connection_when_all_cache_slots_are_occupied() {
    let (owner, _, exchange) = setup(17);
    exchange.immediate.store(true, Ordering::SeqCst);
    let stopped = Arc::new(Semaphore::new(0));
    let mut peers = Vec::new();
    let mut waiters = Vec::new();
    for peer in 1..=16 {
        let (connection, send) = connection(peer.into(), peer);
        waiters.push(tokio::spawn(owner.serve(connection.clone())));
        stream(&send, &stopped).await;
        entered(&exchange.entered, 1).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        peers.push((connection, send));
    }
    assert!(peers.iter().all(|(connection, _)| !connection.is_closed()));
    let (new_peer, send) = connection(17, 17);
    waiters.push(tokio::spawn(owner.serve(new_peer.clone())));
    stream(&send, &stopped).await;
    entered(&exchange.entered, 1).await;
    assert!(!new_peer.is_closed());
    assert!(
        peers.iter().any(|(connection, _)| connection.is_closed()),
        "idle cache did not make room for new peer"
    );
    assert_eq!(exchange.deadlines.lock().expect("request count").len(), 17);
    owner
        .shutdown(BlobShutdownWait::Complete)
        .await
        .expect("shutdown");
    for waiter in waiters {
        waiter.await.expect("service task").expect("service");
    }
    assert_eq!(exchange.closed.load(Ordering::SeqCst), 1);
}
