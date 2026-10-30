//! 普通保留连接重连与显式退休分别维持原关闭、名额及截止责任。
use super::{Admission, Backend};
use crate::connectivity::{
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, IdleRetentionPolicy, PeerConnection,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;

fn fixture() -> (ConnectivityManager, Arc<Backend>) {
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
            idle_retention: Some(IdleRetentionPolicy {
                timeout: Duration::from_secs(30),
                capacity: 8,
                elapsed_clock: None,
            }),
            ..ConnectivityPolicy::default()
        },
    );
    (manager, backend)
}

/// 跨轮保留的连接主动关闭后，新请求须在原期限内等待原名额真实结算。
#[tokio::test(start_paused = true)]
async fn reconnecting_a_retained_connection_waits_for_actual_close_and_local_work() {
    for close_first in [false, true] {
        let (manager, backend) = fixture();
        backend.finish.add_permits(1);
        let lease = manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new(),
            )
            .await
            .expect("initial connection");
        assert!(manager
            .retain(&lease)
            .await
            .expect("retain original connection"));
        let stream = lease
            .open_bi(Instant::now() + Duration::from_secs(1))
            .await
            .expect("local work");
        let (io, work) = stream.into_parts();
        drop(io);
        let original = backend.connections.lock().unwrap()[0].clone();
        original.close();
        drop(lease);
        let request = manager.request(DeviceId::new("peer"));
        let cancellation = CancellationToken::new();
        let next = request.connect(Instant::now() + Duration::from_secs(1), &cancellation);
        tokio::pin!(next);
        assert!(
            timeout(Duration::from_millis(20), &mut next).await.is_err(),
            "retained close must settle before redial instead of returning transient Busy"
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        if close_first {
            original.finished.send_replace(true);
            assert!(timeout(Duration::from_millis(20), &mut next).await.is_err());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            drop(work);
        } else {
            drop(work);
            assert!(timeout(Duration::from_millis(20), &mut next).await.is_err());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            original.finished.send_replace(true);
        }
        backend.finish.add_permits(1);
        let replacement = next.await.expect("redial after original settlement");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
        backend.connections.lock().unwrap()[1]
            .finished
            .send_replace(true);
        drop(replacement);
        manager.shutdown().await.expect("complete shutdown");
    }
}

#[tokio::test(start_paused = true)]
async fn retained_reconnect_wait_keeps_original_deadline_cancellation_and_session() {
    for ending in ["deadline", "cancel", "session"] {
        let (manager, backend) = fixture();
        backend.finish.add_permits(1);
        let lease = manager
            .request(DeviceId::new("peer"))
            .connect(
                Instant::now() + Duration::from_secs(1),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(manager.retain(&lease).await.unwrap());
        let original = backend.connections.lock().unwrap()[0].clone();
        original.close();
        drop(lease);
        let request = manager.request(DeviceId::new("peer"));
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut waiting = tokio::spawn(async move { request.connect(deadline, &token).await });
        assert!(timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err());
        let resetting = match ending {
            "deadline" => {
                tokio::time::advance(Duration::from_secs(1)).await;
                None
            }
            "cancel" => {
                cancel.cancel();
                None
            }
            "session" => {
                let owner = manager.clone();
                Some(tokio::spawn(async move { owner.disconnect_all().await }))
            }
            _ => unreachable!(),
        };
        let error = waiting.await.unwrap().err().expect("old wait must finish");
        if ending == "deadline" {
            assert!(matches!(error, ConnectivityError::TimedOut(_)));
        } else {
            assert_eq!(error.class(), "connection_stopped");
        }
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "waiting cannot dial before old close"
        );
        original.finished.send_replace(true);
        if let Some(resetting) = resetting {
            resetting.await.unwrap().unwrap();
        }
        manager.shutdown().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_retirement_keeps_busy_while_original_work_holds_the_direction() {
    let (manager, backend) = fixture();
    backend.finish.add_permits(1);
    let lease = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(manager.retain(&lease).await.unwrap());
    let stream = lease
        .open_bi(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    let (io, work) = stream.into_parts();
    drop(io);
    let original = backend.connections.lock().unwrap()[0].clone();
    manager.retire(&lease);
    let error = manager
        .request(DeviceId::new("peer"))
        .connect(
            Instant::now() + Duration::from_secs(1),
            &CancellationToken::new(),
        )
        .await
        .err()
        .expect("explicit retirement cannot create a replacement while work remains");
    assert_eq!(error.class(), "connection_busy");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    original.finished.send_replace(true);
    drop(work);
    drop(lease);
    manager.shutdown().await.unwrap();
}
