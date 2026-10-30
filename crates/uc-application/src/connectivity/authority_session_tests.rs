//! 租约当前资格读取等待实际结束，但不能把会话退休前的读取成功交给退休后的调用方。
use super::{Backend, TestConnection};
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectionLease, ConnectionMode, ConnectivityError,
    ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation, PeerConnection,
    SessionConnectionAdmissionPort,
};
use async_trait::async_trait;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

struct ReadGate {
    held: AtomicBool,
    fail: AtomicBool,
    reads: AtomicUsize,
    entered: Semaphore,
    finish: Semaphore,
}
impl ReadGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            held: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            finish: Semaphore::new(0),
        })
    }
    async fn read(&self) -> Result<(), ConnectivityError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.held.load(Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.finish.acquire().await.unwrap().forget();
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(ConnectivityError::Admission(Box::new(io::Error::other(
                "test authority read failed",
            ))));
        }
        Ok(())
    }
}
struct Grant {
    peer: DeviceId,
    gate: Arc<ReadGate>,
}
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.peer
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.gate.read().await
    }
}
struct Attempt {
    peer: DeviceId,
    request: Arc<ReadGate>,
    grant: Arc<ReadGate>,
}
#[async_trait]
impl AdmissionAttempt for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.request.read().await
    }
    async fn adopt(
        &self,
        _: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.check_current().await?;
        Ok(Box::new(Grant {
            peer: self.peer,
            gate: self.grant.clone(),
        }))
    }
}
struct Admission {
    request: Arc<ReadGate>,
    grant: Arc<ReadGate>,
}
#[async_trait]
impl SessionConnectionAdmissionPort for Admission {
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            peer: target,
            request: self.request.clone(),
            grant: self.grant.clone(),
        })
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        _: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        panic!("pending fixture supplies its authenticated grant directly");
    }
}
async fn inbound(manager: &ConnectivityManager, gate: Arc<ReadGate>, id: usize) -> ConnectionLease {
    let raw = Arc::new(TestConnection {
        id,
        mode: ConnectionMode::Multiplexed,
        closed: watch::channel(false).0,
    });
    let mut pending = manager
        .accept_pending(raw, Instant::now() + Duration::from_secs(5))
        .await
        .unwrap();
    let peer = DeviceId::new("current-lease-peer");
    pending.reserve_peer(peer).unwrap();
    let (lease, first) = pending.bind(Box::new(Grant { peer, gate })).await.unwrap();
    drop(first);
    lease
}
#[derive(Clone, Copy)]
enum Case {
    InboundGrant,
    OutboundRequest,
    InboundFailure,
    ShutdownGrant,
}
async fn read_cannot_complete_in_another_session(case: Case) {
    timeout(Duration::from_secs(10), async {
        let request = ReadGate::new();
        let grant = ReadGate::new();
        let backend = Arc::new(Backend {
            gate: Semaphore::new(1),
            calls: AtomicUsize::new(0),
            mode: ConnectionMode::Multiplexed,
            connections: Mutex::new(Vec::new()),
        });
        let manager = ConnectivityManager::new(
            backend,
            Arc::new(Admission {
                request: request.clone(),
                grant: grant.clone(),
            }),
            ConnectivityPolicy::default(),
        );
        let outbound = matches!(case, Case::OutboundRequest);
        let lease = if outbound {
            manager
                .request(DeviceId::new("current-lease-peer"))
                .connect(
                    Instant::now() + Duration::from_secs(5),
                    &CancellationToken::new(),
                )
                .await
                .unwrap()
        } else {
            inbound(&manager, grant.clone(), 1).await
        };
        let held = if outbound {
            request.clone()
        } else {
            grant.clone()
        };
        held.held.store(true, Ordering::SeqCst);
        let checking = tokio::spawn(async move { lease.check_current().await });
        held.entered.acquire().await.unwrap().forget();
        let grants_before = grant.reads.load(Ordering::SeqCst);
        if matches!(case, Case::ShutdownGrant) {
            manager.shutdown().await.unwrap();
        } else {
            manager.disconnect_all().await.unwrap();
        }
        assert!(
            !checking.is_finished(),
            "the actual authority read must still be supervised"
        );
        if matches!(case, Case::InboundFailure) {
            held.fail.store(true, Ordering::SeqCst);
        }
        held.held.store(false, Ordering::SeqCst);
        held.finish.add_permits(1);
        let result = checking.await.unwrap();
        let grants_after = grant.reads.load(Ordering::SeqCst);
        let fresh = if !matches!(case, Case::ShutdownGrant | Case::InboundFailure) {
            let new = inbound(&manager, grant.clone(), 2).await;
            Some(new.check_current().await)
        } else {
            None
        };
        manager.shutdown().await.unwrap();
        if matches!(case, Case::InboundFailure) {
            let error = result.unwrap_err();
            assert!(matches!(&error, ConnectivityError::Admission(_)));
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .kind(),
                io::ErrorKind::Other
            );
        } else {
            assert!(
                matches!(result, Err(ConnectivityError::Stopped)),
                "an old lease returned successful authority after its session ended"
            );
        }
        if outbound {
            assert_eq!(
                grants_after, grants_before,
                "a retired request started another local grant read"
            );
        }
        if let Some(result) = fresh {
            assert!(result.is_ok(), "a new session must still admit a new lease");
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn inflight_inbound_grant_read_cannot_succeed_after_session_restart() {
    read_cannot_complete_in_another_session(Case::InboundGrant).await;
}
#[tokio::test]
async fn inflight_outbound_request_read_cannot_begin_grant_after_session_restart() {
    read_cannot_complete_in_another_session(Case::OutboundRequest).await;
}
#[tokio::test]
async fn inflight_grant_failure_keeps_its_source_after_session_restart() {
    read_cannot_complete_in_another_session(Case::InboundFailure).await;
}
#[tokio::test]
async fn inflight_grant_read_cannot_succeed_after_manager_shutdown() {
    read_cannot_complete_in_another_session(Case::ShutdownGrant).await;
}
