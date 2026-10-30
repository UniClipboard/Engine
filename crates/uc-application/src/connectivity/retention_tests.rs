use super::{fixture, ConnectionMode, ConnectivityPolicy, Ordering};
use crate::connectivity::{
    ElapsedClockError, ElapsedClockPort, IdleRetentionPolicy, PeerConnection,
};
use std::error::Error;
use std::io;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        match self.0.load(Ordering::SeqCst) {
            u64::MAX => Err(ElapsedClockError {
                source: Box::new(io::Error::other("synthetic clock failure")),
            }),
            now => Ok(Duration::from_secs(now)),
        }
    }
}
fn policy(clock: Arc<Clock>) -> ConnectivityPolicy {
    ConnectivityPolicy {
        idle_retention: Some(IdleRetentionPolicy {
            timeout: Duration::from_secs(30),
            capacity: 16,
            elapsed_clock: Some(clock),
        }),
        ..ConnectivityPolicy::default()
    }
}
async fn tick() {
    tokio::time::advance(Duration::from_millis(100)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn suspend_consumes_idle_budget_without_advancing_tokio_thirty_seconds() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy(clock.clone()));
    backend.gate.add_permits(1);
    let lease = manager
        .request(DeviceId::new("idle-peer"))
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&lease).await.unwrap());
    drop(lease);
    tokio::task::yield_now().await;
    clock.0.store(30, Ordering::SeqCst);
    tick().await;
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn expired_connection_cannot_be_reused_before_the_idle_worker_runs() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy(clock.clone()));
    backend.gate.add_permits(2);
    let peer = DeviceId::new("idle-peer");
    let first = manager
        .request(peer)
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let old_id = first.connection_id();
    assert!(manager.retain(&first).await.unwrap());
    drop(first);
    // 不advance、不yield：请求必须自己检查原生截止，不能等后台回收先执行。
    clock.0.store(30, Ordering::SeqCst);
    let second = manager
        .request(peer)
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_ne!(second.connection_id(), old_id);
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    drop(second);
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn real_work_stays_active_and_idle_budget_starts_only_after_its_last_lease() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy(clock.clone()));
    backend.gate.add_permits(1);
    let lease = manager
        .request(DeviceId::new("active-peer"))
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&lease).await.unwrap());
    let stream = lease
        .open_bi(Instant::now() + Duration::from_secs(5))
        .await
        .unwrap();
    let (io, work) = stream.into_parts();
    drop(io);
    drop(lease);
    clock.0.store(u64::MAX, Ordering::SeqCst);
    tick().await;
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    clock.0.store(120, Ordering::SeqCst);
    drop(work);
    tokio::task::yield_now().await;
    clock.0.store(149, Ordering::SeqCst);
    tick().await;
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    clock.0.store(150, Ordering::SeqCst);
    tick().await;
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unavailable_idle_clock_retires_the_cache_and_preserves_the_source() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy(clock.clone()));
    backend.gate.add_permits(1);
    let peer = DeviceId::new("idle-peer");
    let lease = manager
        .request(peer)
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&lease).await.unwrap());
    drop(lease);
    clock.0.store(u64::MAX, Ordering::SeqCst);
    let error = manager
        .request(peer)
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .err()
        .expect("clock unavailable");
    assert_eq!(error.class(), "elapsed_clock_unavailable");
    let mut source = error.source();
    let mut found_io = false;
    while let Some(error) = source {
        found_io |= error.is::<io::Error>();
        source = error.source();
    }
    assert!(found_io);
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn regressing_idle_reading_cannot_renew_the_old_budget() {
    let clock = Arc::new(Clock(AtomicU64::new(10)));
    let (manager, backend, _) = fixture(ConnectionMode::Multiplexed, policy(clock.clone()));
    backend.gate.add_permits(1);
    let lease = manager
        .request(DeviceId::new("idle-peer"))
        .connect(
            Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&lease).await.unwrap());
    drop(lease);
    tokio::task::yield_now().await;
    clock.0.store(20, Ordering::SeqCst);
    tick().await;
    assert!(!backend.connections.lock().unwrap()[0].is_closed());
    clock.0.store(19, Ordering::SeqCst);
    tick().await;
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    clock.0.store(21, Ordering::SeqCst);
    tick().await;
    assert!(backend.connections.lock().unwrap()[0].is_closed());
    manager.shutdown().await.unwrap();
}
