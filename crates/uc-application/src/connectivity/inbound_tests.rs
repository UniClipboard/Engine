//! 通过真正的准入 future 观察容量交接，不把逻辑超时当作本地读取完成。
use super::lifetime::ClosingConnection;
use super::{fixture, Attempt, Backend, Grant, InboundFence, TestConnection};
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectivityError, ConnectivityManager,
    ConnectivityPolicy, InboundAdmissionFence, InboundAdmissionPolicy, InboundIdentityReservation,
    PeerConnection, SessionConnectionAdmissionPort,
};
use async_trait::async_trait;
use futures::poll;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Semaphore};
use tokio::time::{advance, timeout};
use uc_core::ids::DeviceId;

struct Admission {
    identity_entered: Semaphore,
    identity_finish: Semaphore,
    ledger_entered: Semaphore,
    ledger_finish: Semaphore,
    epoch: Arc<AtomicUsize>,
    fail: AtomicBool,
    panic: AtomicBool,
}
#[async_trait]
impl SessionConnectionAdmissionPort for Admission {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        Some(Box::new(InboundFence {
            epoch: self.epoch.load(Ordering::SeqCst),
            current: self.epoch.clone(),
        }))
    }
    fn begin(&self, peer: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            epoch: self.epoch.load(Ordering::SeqCst),
            current: self.epoch.clone(),
            peer,
        })
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.identity_entered.add_permits(1);
        self.identity_finish
            .acquire()
            .await
            .expect("identity finish")
            .forget();
        let peer = DeviceId::new(if connection.id() < 100 {
            "first"
        } else {
            "second"
        });
        reservation.identified(peer)?;
        self.ledger_entered.add_permits(1);
        self.ledger_finish
            .acquire()
            .await
            .expect("ledger finish")
            .forget();
        assert!(
            !self.panic.load(Ordering::SeqCst),
            "synthetic admission worker panic"
        );
        if self.fail.load(Ordering::SeqCst) {
            return Err(ConnectivityError::Admission(Box::new(io::Error::from(
                io::ErrorKind::PermissionDenied,
            ))));
        }
        Ok(Box::new(Grant {
            epoch: self.epoch.load(Ordering::SeqCst),
            current: self.epoch.clone(),
            peer,
        }))
    }
}
fn setup(direction_limit: Option<usize>) -> (ConnectivityManager, Arc<Admission>, Arc<Backend>) {
    let (_, backend, epoch) = fixture(ConnectionMode::Multiplexed, ConnectivityPolicy::default());
    let admission = Arc::new(Admission {
        identity_entered: Semaphore::new(0),
        identity_finish: Semaphore::new(0),
        ledger_entered: Semaphore::new(0),
        ledger_finish: Semaphore::new(0),
        epoch,
        fail: AtomicBool::new(false),
        panic: AtomicBool::new(false),
    });
    let manager = ConnectivityManager::new(
        backend.clone(),
        admission.clone(),
        ConnectivityPolicy {
            connections_per_peer_direction: direction_limit,
            inbound_admission: Some(InboundAdmissionPolicy {
                unidentified: 16,
                identified_per_peer: 2,
                timeout: Duration::from_secs(60),
            }),
            ..ConnectivityPolicy::default()
        },
    );
    (manager, admission, backend)
}
fn raw(id: usize) -> Arc<TestConnection> {
    Arc::new(TestConnection {
        id,
        mode: ConnectionMode::Multiplexed,
        closed: watch::channel(false).0,
    })
}
async fn entered(signal: &Semaphore, count: u32) {
    timeout(Duration::from_secs(1), signal.acquire_many(count))
        .await
        .expect("work did not start")
        .expect("work signal")
        .forget();
}

#[tokio::test(start_paused = true)]
async fn unknown_admission_has_sixteen_slots_and_dropped_waiters_do_not_release_started_reads() {
    let (manager, admission, _backend) = setup(None);
    let mut pending = Vec::new();
    for id in 0..16 {
        pending.push(manager.accept(raw(id)));
    }
    entered(&admission.identity_entered, 16).await;
    let rejected = raw(16);
    assert!(manager
        .accept(rejected.clone())
        .await
        .err()
        .expect("seventeenth accepted")
        .is_busy());
    assert!(rejected.is_closed());
    drop(pending.remove(0));
    let rejected = raw(17);
    assert!(manager
        .accept(rejected)
        .await
        .err()
        .expect("unsettled read released capacity")
        .is_busy());
    drop(pending);
    admission.identity_finish.add_permits(16);
    admission.ledger_finish.add_permits(16);
    manager.shutdown().await.expect("settle all accepted reads");
    assert_eq!(manager.inner.incoming.available_permits(), 16);
}

#[tokio::test(start_paused = true)]
async fn identified_admission_is_two_per_peer_and_returns_the_unknown_slots_before_ledger_completion(
) {
    let (manager, admission, _backend) = setup(None);
    admission.identity_finish.add_permits(2);
    let first = manager.accept(raw(1));
    let second = manager.accept(raw(2));
    entered(&admission.ledger_entered, 2).await;
    admission.identity_finish.add_permits(1);
    let excess = raw(3);
    assert!(manager
        .accept(excess.clone())
        .await
        .err()
        .expect("third identified read accepted")
        .is_busy());
    assert!(excess.is_closed());
    admission.identity_finish.add_permits(1);
    let another_peer = manager.accept(raw(100));
    entered(&admission.ledger_entered, 1).await;
    let mut unknown = Vec::new();
    for id in 20..36 {
        unknown.push(manager.accept(raw(id)));
    }
    let rejected = manager
        .accept(raw(36))
        .await
        .err()
        .expect("unknown capacity was not bounded");
    assert!(rejected.is_busy());
    admission.ledger_finish.add_permits(3);
    let first = first.await.expect("first peer");
    let second = second.await.expect("second read");
    let other = another_peer.await.expect("another peer");
    assert_eq!(other.peer(), DeviceId::new("second"));
    drop((first, second, other, unknown));
    admission.identity_finish.add_permits(16);
    admission.ledger_finish.add_permits(16);
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn admission_freezes_revocation_before_identity_and_refuses_a_rejoined_old_request() {
    let (manager, admission, _backend) = setup(None);
    let old_raw = raw(1);
    let old = manager.accept(old_raw.clone());
    entered(&admission.identity_entered, 1).await;
    admission.epoch.fetch_add(2, Ordering::SeqCst);
    admission.identity_finish.add_permits(1);
    assert!(old.await.is_err());
    assert!(old_raw.is_closed());
    assert_eq!(admission.ledger_entered.available_permits(), 0);
    admission.identity_finish.add_permits(1);
    admission.ledger_finish.add_permits(1);
    drop(
        manager
            .accept(raw(2))
            .await
            .expect("fresh membership request"),
    );
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn admission_deadline_stops_the_connection_but_waits_for_real_read_and_keeps_its_failure() {
    let (manager, admission, _backend) = setup(None);
    admission.identity_finish.add_permits(1);
    admission.fail.store(true, Ordering::SeqCst);
    let connection = raw(1);
    let mut result = Box::pin(manager.accept(connection.clone()));
    entered(&admission.ledger_entered, 1).await;
    advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert!(
        connection.is_closed(),
        "timeout did not stop the actual connection"
    );
    assert!(
        poll!(&mut result).is_pending(),
        "logical timeout released local read"
    );
    admission.ledger_finish.add_permits(1);
    let error = result.await.err().expect("late failed read accepted");
    let source = error
        .source()
        .expect("connectivity source")
        .downcast_ref::<ConnectivityError>()
        .expect("typed connectivity error");
    let ConnectivityError::Settlement {
        primary,
        additional,
    } = source
    else {
        panic!("compound source missing");
    };
    assert!(matches!(
        primary.as_ref(),
        ConnectivityError::AdmissionTimedOut(_)
    ));
    assert_eq!(
        additional
            .source()
            .expect("read error")
            .downcast_ref::<io::Error>()
            .expect("original IO error")
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn session_close_joins_abandoned_admission_and_old_request_does_not_start_ledger_after_reactivation(
) {
    let (manager, admission, _backend) = setup(None);
    let connection = raw(1);
    let old = manager.accept(connection.clone());
    entered(&admission.identity_entered, 1).await;
    let owner = manager.clone();
    let mut closing = tokio::spawn(async move { owner.disconnect_all().await });
    tokio::task::yield_now().await;
    assert!(connection.is_closed());
    assert!(
        poll!(&mut closing).is_pending(),
        "session close lost accepted local work"
    );
    admission.identity_finish.add_permits(1);
    closing.await.expect("close task").expect("close session");
    assert!(old.await.is_err());
    assert_eq!(admission.ledger_entered.available_permits(), 0);
    admission.identity_finish.add_permits(1);
    admission.ledger_finish.add_permits(1);
    drop(manager.accept(raw(2)).await.expect("reactivated session"));
    manager.shutdown().await.expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn admission_panic_is_returned_with_its_source_and_retained_for_shutdown() {
    let (manager, admission, _backend) = setup(None);
    admission.identity_finish.add_permits(1);
    admission.ledger_finish.add_permits(1);
    admission.panic.store(true, Ordering::SeqCst);
    let connection = raw(1);
    let error = manager
        .accept(connection.clone())
        .await
        .err()
        .expect("panic succeeded");
    let source = error
        .source()
        .expect("task error")
        .downcast_ref::<ConnectivityError>()
        .expect("connectivity error");
    assert!(matches!(source, ConnectivityError::Task(task) if task.is_panic()));
    assert!(connection.is_closed());
    assert!(
        matches!(manager.shutdown().await, Err(ConnectivityError::Task(task)) if task.is_panic())
    );
}

#[tokio::test(start_paused = true)]
async fn incoming_direction_is_independent_and_keeps_its_slot_until_work_and_close_both_finish() {
    for close_first in [false, true] {
        let (manager, admission, backend) = setup(Some(1));
        admission.identity_finish.add_permits(1);
        admission.ledger_finish.add_permits(1);
        let connection = ClosingConnection::new(1);
        let lease = manager
            .accept(connection.clone())
            .await
            .expect("incoming connection");
        backend.gate.add_permits(1);
        let outgoing = manager
            .request(DeviceId::new("first"))
            .connect(
                tokio::time::Instant::now() + Duration::from_secs(1),
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("incoming slot must not block outgoing direction");
        drop(outgoing);
        let stream = lease
            .open_bi(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .expect("local work");
        let (io, work) = stream.into_parts();
        drop(io);
        lease.close();
        drop(lease);
        let mut retained_work = Some(work);
        if close_first {
            connection.finished.send_replace(true);
        } else {
            retained_work.take();
        }
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        admission.identity_finish.add_permits(1);
        let rejected = raw(2);
        let error = manager
            .accept(rejected.clone())
            .await
            .err()
            .expect("second incoming connection accepted");
        assert!(error.is_busy());
        assert!(rejected.is_closed());
        drop(error);
        retained_work.take();
        connection.finished.send_replace(true);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        admission.identity_finish.add_permits(1);
        admission.ledger_finish.add_permits(1);
        drop(
            manager
                .accept(raw(3))
                .await
                .expect("incoming direction released"),
        );
        manager.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn single_exchange_busy_releases_capacity_only_after_stream_is_dropped() {
    let (_unused, admission, backend) = setup(None);
    let manager = ConnectivityManager::new(
        backend,
        admission.clone(),
        ConnectivityPolicy {
            inbound_per_session: 1,
            inbound_per_connection: 1,
            reject_excess_inbound: true,
            ..ConnectivityPolicy::default()
        },
    );
    admission.identity_finish.add_permits(2);
    admission.ledger_finish.add_permits(2);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let (connection, stream) = manager.accept_exchange(raw(1), deadline).await.unwrap();
    let excess = raw(2);
    let result = timeout(
        Duration::from_millis(100),
        manager.accept_exchange(excess.clone(), deadline),
    )
    .await
    .expect("超额请求不能排队");
    assert!(matches!(result, Err(ConnectivityError::Busy(_))));
    assert!(excess.is_closed());
    drop(connection);
    let still_busy = raw(3);
    assert!(matches!(
        manager.accept_exchange(still_busy, deadline).await,
        Err(ConnectivityError::Busy(_))
    ));
    drop(stream);
    let (next, next_stream) = manager.accept_exchange(raw(4), deadline).await.unwrap();
    drop(next_stream);
    drop(next);
    manager.shutdown().await.unwrap();
}
