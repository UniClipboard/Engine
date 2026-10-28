//! 第二 backend 的两阶段恢复通信；内存流不代表真实网络或密码握手。
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::poll;
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, Semaphore},
    time::{timeout, Instant},
};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, BeginMembershipBranchRecoveryInput,
    BranchRecoveryLocalWorkPort, BranchRecoveryReceiver, BranchRecoveryRequest,
    BranchRecoveryResponse, BranchRecoveryTransport, BranchRecoveryWirePort, BranchRecoveryWork,
    ConnectionProtocol, ConnectivityBackend, ConnectivityError, ConnectivityManager,
    ConnectivityPolicy, InboundAdmissionFence, InboundIdentityReservation,
    IssueMembershipBranchRecoveryError, IssueMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryPort, MembershipBranchRecoveryChannelError,
    MembershipBranchRecoveryChannelPort, MembershipBranchRecoveryCommit,
    MembershipBranchRecoveryRequest, PeerConnection, SessionConnectionAdmissionPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1, MembershipConflictId,
};

use super::stream::memory_backend_fixture::{recovery_connection_pair, Members};
use super::BranchRecoveryWire;

struct MemoryDial {
    target: DeviceId,
    second_target: Option<DeviceId>,
    incoming: mpsc::Sender<Arc<dyn PeerConnection>>,
    dials: AtomicUsize,
}

struct SlowDial {
    inner: MemoryDial,
    started: Arc<Semaphore>,
}

#[async_trait]
impl ConnectivityBackend for SlowDial {
    async fn connect(
        &self,
        target: DeviceId,
        hint: Option<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        self.started.add_permits(1);
        tokio::time::sleep(Duration::from_secs(20)).await;
        self.inner.connect(target, hint, cancel).await
    }
}

#[tokio::test(start_paused = true)]
async fn recovery_exchange_deadline_is_not_renewed_during_response_prefix() -> anyhow::Result<()> {
    assert_response_deadline(false).await
}

#[tokio::test(start_paused = true)]
async fn recovery_exchange_deadline_is_not_renewed_during_response_body() -> anyhow::Result<()> {
    assert_response_deadline(true).await
}

async fn assert_response_deadline(partial_body: bool) -> anyhow::Result<()> {
    let peer = DeviceId::new("deadline-peer");
    let (incoming, mut connections) = mpsc::channel(1);
    let dial_started = Arc::new(Semaphore::new(0));
    let backend = Arc::new(SlowDial {
        inner: MemoryDial {
            target: peer,
            second_target: None,
            incoming,
            dials: AtomicUsize::new(0),
        },
        started: dial_started.clone(),
    });
    let manager = ConnectivityManager::new(
        backend,
        Arc::new(Members {
            peer,
            key: [2; 32],
            allowed: Arc::new(AtomicBool::new(true)),
        }),
        ConnectivityPolicy::default(),
    );
    let transport = BranchRecoveryTransport::new(manager.clone(), Arc::new(BranchRecoveryWire));
    let scope = transport.freeze(peer)?;
    let started = Instant::now();
    let exchange = tokio::spawn(async move {
        scope
            .exchange(BranchRecoveryRequest::GroupInfo {
                conflict_id: MembershipConflictId::from_bytes([1; 32]),
                target_branch_id: MembershipBranchId::from_bytes([2; 32]),
                recipient_member: MemberInstanceId::from_bytes([3; 32]),
            })
            .await
    });
    dial_started.acquire().await?.forget();
    tokio::time::advance(Duration::from_secs(20)).await;
    let unanswered = connections
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("missing dial"))?;
    let (mut send, mut receive) = unanswered.accept_bi().await?;
    let received = BranchRecoveryWire
        .read_request(receive.as_mut(), started + Duration::from_secs(30))
        .await?;
    assert!(matches!(received, BranchRecoveryRequest::GroupInfo { .. }));
    if partial_body {
        send.write_u32(2).await?;
        send.write_all(&[0]).await?;
    } else {
        send.write_all(&[0, 0]).await?;
    }
    send.flush().await?;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(!exchange.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    let result = exchange.await?;
    assert!(matches!(
        result,
        Err(MembershipBranchRecoveryChannelError::Unavailable { .. })
    ));
    assert_eq!(
        Instant::now().duration_since(started),
        Duration::from_secs(30)
    );
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn recovery_exchange_deadline_includes_waiting_for_peer_capacity() -> anyhow::Result<()> {
    let peer = DeviceId::new("deadline-peer");
    let (incoming, mut connections) = mpsc::channel(2);
    let backend = Arc::new(MemoryDial {
        target: peer,
        second_target: None,
        incoming,
        dials: AtomicUsize::new(0),
    });
    let manager = ConnectivityManager::new(
        backend,
        Arc::new(Members {
            peer,
            key: [2; 32],
            allowed: Arc::new(AtomicBool::new(true)),
        }),
        ConnectivityPolicy {
            outbound_per_peer: 1,
            ..ConnectivityPolicy::default()
        },
    );
    let transport = BranchRecoveryTransport::new(manager.clone(), Arc::new(BranchRecoveryWire));
    let first_scope = transport.freeze(peer)?;
    let second_scope = transport.freeze(peer)?;
    let message = || BranchRecoveryRequest::GroupInfo {
        conflict_id: MembershipConflictId::from_bytes([1; 32]),
        target_branch_id: MembershipBranchId::from_bytes([2; 32]),
        recipient_member: MemberInstanceId::from_bytes([3; 32]),
    };
    let first_message = message();
    let first = tokio::spawn(async move { first_scope.exchange(first_message).await });
    let first_connection = connections
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("missing first dial"))?;
    let (_first_send, mut first_receive) = first_connection.accept_bi().await?;
    BranchRecoveryWire
        .read_request(
            first_receive.as_mut(),
            Instant::now() + Duration::from_secs(30),
        )
        .await?;

    let started = Instant::now();
    let second_message = message();
    let second = tokio::spawn(async move { second_scope.exchange(second_message).await });
    let second_connection = connections
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("missing second dial"))?;
    let mut accepting = Box::pin(second_connection.accept_bi());
    tokio::task::yield_now().await;
    assert!(poll!(&mut accepting).is_pending());
    tokio::time::advance(Duration::from_secs(20)).await;
    first.abort();
    assert!(matches!(first.await, Err(error) if error.is_cancelled()));
    let (_send, mut receive) = accepting.await?;
    BranchRecoveryWire
        .read_request(receive.as_mut(), started + Duration::from_secs(30))
        .await?;
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(!second.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(
        second.await?,
        Err(MembershipBranchRecoveryChannelError::Unavailable { .. })
    ));
    assert_eq!(
        Instant::now().duration_since(started),
        Duration::from_secs(30)
    );
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn first_stream_deadline_includes_inbound_identity_admission() -> anyhow::Result<()> {
    struct SlowAdmission(Members);

    #[async_trait]
    impl SessionConnectionAdmissionPort for SlowAdmission {
        fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
            self.0.inbound_fence()
        }

        fn begin(&self, peer: DeviceId) -> Box<dyn AdmissionAttempt> {
            self.0.begin(peer)
        }

        async fn adopt_inbound(
            &self,
            connection: Arc<dyn PeerConnection>,
            reservation: &dyn InboundIdentityReservation,
        ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
            tokio::time::sleep(Duration::from_secs(7)).await;
            self.0.adopt_inbound(connection, reservation).await
        }
    }

    let peer = DeviceId::new("deadline-peer");
    let (incoming, _connections) = mpsc::channel(1);
    let manager = ConnectivityManager::new(
        Arc::new(MemoryDial {
            target: peer,
            second_target: None,
            incoming,
            dials: AtomicUsize::new(0),
        }),
        Arc::new(SlowAdmission(Members {
            peer,
            key: [1; 32],
            allowed: Arc::new(AtomicBool::new(true)),
        })),
        ConnectivityPolicy::default(),
    );
    let (_sender, receiver) = recovery_connection_pair(1, [2; 32]);
    let started = Instant::now();
    let mut accepting =
        Box::pin(manager.accept_exchange(receiver.clone(), started + Duration::from_secs(10)));
    assert!(poll!(&mut accepting).is_pending());
    tokio::time::advance(Duration::from_secs(7)).await;
    assert!(poll!(&mut accepting).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(poll!(&mut accepting).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(
        accepting.await,
        Err(ConnectivityError::TimedOut(_))
    ));
    assert_eq!(
        Instant::now().duration_since(started),
        Duration::from_secs(10)
    );
    assert!(receiver.is_closed());
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn recovery_inbound_capacity_rejects_ninth_and_returns_after_release() -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), async {
        let peer = DeviceId::new("capacity-peer");
        let (incoming, _connections) = mpsc::channel(1);
        let manager = ConnectivityManager::new(
            Arc::new(MemoryDial {
                target: peer,
                second_target: None,
                incoming,
                dials: AtomicUsize::new(0),
            }),
            Arc::new(Members {
                peer,
                key: [1; 32],
                allowed: Arc::new(AtomicBool::new(true)),
            }),
            ConnectivityPolicy {
                outbound_per_peer: 1,
                inbound_per_connection: 1,
                inbound_per_session: 8,
                reject_excess_inbound: true,
                waiters_per_peer: 4,
                waiters_per_session: 16,
                flights_per_session: 4,
                supervise_local_work: true,
                ..ConnectivityPolicy::default()
            },
        );
        let mut held = Vec::new();
        for id in 0..8 {
            let (sender, receiver) = recovery_connection_pair(id, [2; 32]);
            let client_stream = sender.open_bi().await?;
            let (lease, stream) = manager
                .accept_exchange(receiver.clone(), Instant::now() + Duration::from_secs(5))
                .await?;
            held.push((sender, receiver, client_stream, lease, stream));
        }
        let (_sender, rejected) = recovery_connection_pair(8, [2; 32]);
        assert!(matches!(
            manager
                .accept_exchange(rejected.clone(), Instant::now() + Duration::from_secs(5))
                .await,
            Err(ConnectivityError::Busy(_))
        ));
        assert!(rejected.is_closed());
        drop(held.pop());
        let (sender, receiver) = recovery_connection_pair(9, [2; 32]);
        let client_stream = sender.open_bi().await?;
        let (lease, stream) = manager
            .accept_exchange(receiver.clone(), Instant::now() + Duration::from_secs(5))
            .await?;
        held.push((sender, receiver, client_stream, lease, stream));
        manager.shutdown().await?;
        assert!(held
            .iter()
            .all(|(_, receiver, _, _, _)| receiver.is_closed()));
        drop(held);
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[async_trait]
impl ConnectivityBackend for MemoryDial {
    async fn connect(
        &self,
        target: DeviceId,
        _: Option<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if target != self.target && self.second_target != Some(target) {
            return Err(ConnectivityError::AddressMissing);
        }
        let id = self.dials.fetch_add(1, Ordering::SeqCst);
        let key = if target == self.target {
            [2; 32]
        } else {
            [3; 32]
        };
        let (local, remote) = recovery_connection_pair(id, key);
        self.incoming
            .send(remote)
            .await
            // discarded-source[channel]: 测试队列关闭只表示接收方已停止，错误携带的是连接对象。
            .map_err(|_| ConnectivityError::Stopped)?;
        Ok(local)
    }
}

struct InlineLocalWork;

#[derive(Clone)]
struct TwoPeers {
    first: DeviceId,
    second: DeviceId,
    allowed: Arc<AtomicBool>,
}
impl InboundAdmissionFence for TwoPeers {
    fn check_current(&self, peer: &DeviceId) -> Result<(), ConnectivityError> {
        if (*peer == self.first || *peer == self.second) && self.allowed.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for TwoPeers {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        Some(Box::new(self.clone()))
    }
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        let peer = if target == self.first || target == self.second {
            target
        } else {
            self.first
        };
        Members {
            peer,
            key: if target == self.first {
                [2; 32]
            } else {
                [3; 32]
            },
            allowed: self.allowed.clone(),
        }
        .begin(target)
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        _: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        Err(ConnectivityError::Denied)
    }
}
#[async_trait]
impl BranchRecoveryLocalWorkPort for InlineLocalWork {
    async fn execute(
        &self,
        work: BranchRecoveryWork,
    ) -> Result<Result<BranchRecoveryResponse, anyhow::Error>, anyhow::Error> {
        Ok(work.await)
    }
}

struct Issuer {
    peer: DeviceId,
    conflict: MembershipConflictId,
    branch: MembershipBranchId,
    member: MemberInstanceId,
    package: MembershipBranchRecoveryPackageV1,
    calls: AtomicUsize,
    block_first: AtomicBool,
    block_second: AtomicBool,
    started: Semaphore,
    resume: Semaphore,
    second_started: Semaphore,
    second_resume: Semaphore,
}
#[async_trait]
impl IssueMembershipBranchRecoveryPort for Issuer {
    async fn begin_membership_branch_recovery(
        &self,
        input: BeginMembershipBranchRecoveryInput,
    ) -> Result<Vec<u8>, IssueMembershipBranchRecoveryError> {
        assert_eq!(
            (
                input.source_device_id,
                input.conflict_id,
                input.target_branch_id,
                input.recipient_member
            ),
            (self.peer, self.conflict, self.branch, self.member)
        );
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.block_first.swap(false, Ordering::SeqCst) {
            self.started.add_permits(1);
            self.resume.acquire().await.unwrap().forget();
        } else if call == 1 && self.block_second.swap(false, Ordering::SeqCst) {
            self.second_started.add_permits(1);
            self.second_resume.acquire().await.unwrap().forget();
        }
        Ok(vec![0x41, 0x42])
    }
    async fn issue_membership_branch_recovery(
        &self,
        input: IssueMembershipBranchRecoveryInput,
    ) -> Result<MembershipBranchRecoveryPackageV1, IssueMembershipBranchRecoveryError> {
        assert_eq!(
            (
                input.source_device_id,
                input.conflict_id,
                input.target_branch_id,
                input.recipient_member
            ),
            (self.peer, self.conflict, self.branch, self.member)
        );
        assert_eq!(input.external_commit, vec![0x43]);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.package.clone())
    }
}

#[tokio::test]
async fn memory_backend_runs_both_managed_recovery_exchanges() -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), async {
        let sender_id = DeviceId::new("memory-recipient");
        let receiver_id = DeviceId::new("memory-issuer");
        let allowed = Arc::new(AtomicBool::new(true));
        let (incoming, mut connections) = mpsc::channel(4);
        let backend = Arc::new(MemoryDial {
            target: receiver_id,
            second_target: None,
            incoming,
            dials: AtomicUsize::new(0),
        });
        let policy = ConnectivityPolicy {
            outbound_per_peer: 1,
            inbound_per_connection: 1,
            inbound_per_session: 8,
            reject_excess_inbound: true,
            ..ConnectivityPolicy::default()
        };
        let sender = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: receiver_id,
                key: [2; 32],
                allowed: allowed.clone(),
            }),
            policy.clone(),
        );
        let receiver = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: sender_id,
                key: [1; 32],
                allowed,
            }),
            policy,
        );
        let conflict = MembershipConflictId::from_bytes([0x51; 32]);
        let branch = MembershipBranchId::from_bytes([0x52; 32]);
        let member = MemberInstanceId::from_bytes([0x53; 32]);
        let package = MembershipBranchRecoveryPackageV1::new_unsigned(
            conflict,
            branch,
            member,
            member,
            1000,
            [0x54; 32],
            vec![1],
            vec![2],
            vec![3],
        )?
        .with_authorization_signature(vec![4]);
        let issuer = Arc::new(Issuer {
            peer: sender_id,
            conflict,
            branch,
            member,
            package: package.clone(),
            calls: AtomicUsize::new(0),
            block_first: AtomicBool::new(false),
            block_second: AtomicBool::new(false),
            started: Semaphore::new(0),
            resume: Semaphore::new(0),
            second_started: Semaphore::new(0),
            second_resume: Semaphore::new(0),
        });
        let wire = Arc::new(BranchRecoveryWire);
        let transport = BranchRecoveryTransport::new(sender.clone(), wire.clone());
        let inbound = BranchRecoveryReceiver::new(wire, Arc::new(InlineLocalWork), issuer.clone());
        let scope = transport.freeze(receiver_id)?;
        let request =
            MembershipBranchRecoveryRequest::for_test(receiver_id, conflict, branch, member, scope);
        let serve = async {
            for _ in 0..2 {
                let raw = connections
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("memory inbound closed"))?;
                assert_eq!(raw.protocol(), ConnectionProtocol::MembershipBranchRecovery);
                let deadline = Instant::now() + Duration::from_secs(5);
                let (connection, stream) = receiver.accept_exchange(raw, deadline).await?;
                inbound.receive(connection, stream, deadline).await?;
            }
            anyhow::Ok(())
        };
        let exchange = async {
            assert_eq!(
                transport
                    .request_membership_branch_group_info(request.clone())
                    .await?,
                vec![0x41, 0x42]
            );
            let received = transport
                .submit_membership_branch_external_commit(MembershipBranchRecoveryCommit {
                    request,
                    external_commit: vec![0x43],
                })
                .await?;
            assert_eq!(received, package);
            anyhow::Ok(())
        };
        tokio::try_join!(serve, exchange)?;
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 2);
        assert_eq!(backend.dials.load(Ordering::SeqCst), 2);
        sender.shutdown().await?;
        receiver.shutdown().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn memory_backend_cancelled_session_does_not_revive_old_scope() -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), async {
        let sender_id = DeviceId::new("memory-recipient");
        let receiver_id = DeviceId::new("memory-issuer");
        let allowed = Arc::new(AtomicBool::new(true));
        let (incoming, mut connections) = mpsc::channel(4);
        let backend = Arc::new(MemoryDial {
            target: receiver_id,
            second_target: None,
            incoming,
            dials: AtomicUsize::new(0),
        });
        let policy = ConnectivityPolicy {
            outbound_per_peer: 1,
            inbound_per_connection: 1,
            inbound_per_session: 8,
            reject_excess_inbound: true,
            ..ConnectivityPolicy::default()
        };
        let sender = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: receiver_id,
                key: [2; 32],
                allowed: allowed.clone(),
            }),
            policy.clone(),
        );
        let receiver = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: sender_id,
                key: [1; 32],
                allowed,
            }),
            policy,
        );
        let conflict = MembershipConflictId::from_bytes([0x51; 32]);
        let branch = MembershipBranchId::from_bytes([0x52; 32]);
        let member = MemberInstanceId::from_bytes([0x53; 32]);
        let package = MembershipBranchRecoveryPackageV1::new_unsigned(
            conflict,
            branch,
            member,
            member,
            1000,
            [0x54; 32],
            vec![1],
            vec![2],
            vec![3],
        )?
        .with_authorization_signature(vec![4]);
        let issuer = Arc::new(Issuer {
            peer: sender_id,
            conflict,
            branch,
            member,
            package,
            calls: AtomicUsize::new(0),
            block_first: AtomicBool::new(true),
            block_second: AtomicBool::new(false),
            started: Semaphore::new(0),
            resume: Semaphore::new(0),
            second_started: Semaphore::new(0),
            second_resume: Semaphore::new(0),
        });
        let wire = Arc::new(BranchRecoveryWire);
        let transport = Arc::new(BranchRecoveryTransport::new(sender.clone(), wire.clone()));
        let inbound = BranchRecoveryReceiver::new(wire, Arc::new(InlineLocalWork), issuer.clone());
        let serving_manager = receiver.clone();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let raw = connections
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("memory inbound closed"))?;
                let deadline = Instant::now() + Duration::from_secs(5);
                let (connection, stream) = serving_manager.accept_exchange(raw, deadline).await?;
                let result = inbound.receive(connection, stream, deadline).await;
                if index == 1 {
                    result?;
                }
            }
            anyhow::Ok(())
        });
        let old_request = MembershipBranchRecoveryRequest::for_test(
            receiver_id,
            conflict,
            branch,
            member,
            transport.freeze(receiver_id)?,
        );
        let waiting_transport = transport.clone();
        let waiting_request = old_request.clone();
        let waiting = tokio::spawn(async move {
            waiting_transport
                .request_membership_branch_group_info(waiting_request)
                .await
        });
        issuer.started.acquire().await?.forget();
        sender.disconnect_all().await?;
        assert!(matches!(
            waiting.await?,
            Err(MembershipBranchRecoveryChannelError::Unavailable { .. })
        ));
        issuer.resume.add_permits(1);
        assert!(matches!(
            transport
                .request_membership_branch_group_info(old_request)
                .await,
            Err(MembershipBranchRecoveryChannelError::Unavailable { .. })
        ));
        assert_eq!(backend.dials.load(Ordering::SeqCst), 1);
        let fresh = MembershipBranchRecoveryRequest::for_test(
            receiver_id,
            conflict,
            branch,
            member,
            transport.freeze(receiver_id)?,
        );
        assert_eq!(
            transport
                .request_membership_branch_group_info(fresh)
                .await?,
            vec![0x41, 0x42]
        );
        server.await??;
        assert_eq!(backend.dials.load(Ordering::SeqCst), 2);
        sender.shutdown().await?;
        receiver.shutdown().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn cancelling_one_memory_exchange_does_not_stop_another() -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), async {
        let sender_id = DeviceId::new("memory-recipient");
        let receiver_id = DeviceId::new("memory-issuer");
        let allowed = Arc::new(AtomicBool::new(true));
        let (incoming, mut connections) = mpsc::channel(4);
        let backend = Arc::new(MemoryDial {
            target: receiver_id,
            second_target: None,
            incoming,
            dials: AtomicUsize::new(0),
        });
        let policy = ConnectivityPolicy {
            outbound_per_peer: 1,
            inbound_per_connection: 1,
            inbound_per_session: 8,
            reject_excess_inbound: true,
            ..ConnectivityPolicy::default()
        };
        let sender = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: receiver_id,
                key: [2; 32],
                allowed: allowed.clone(),
            }),
            policy.clone(),
        );
        let receiver = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: sender_id,
                key: [1; 32],
                allowed,
            }),
            policy,
        );
        let conflict = MembershipConflictId::from_bytes([0x51; 32]);
        let branch = MembershipBranchId::from_bytes([0x52; 32]);
        let member = MemberInstanceId::from_bytes([0x53; 32]);
        let package = MembershipBranchRecoveryPackageV1::new_unsigned(
            conflict,
            branch,
            member,
            member,
            1000,
            [0x54; 32],
            vec![1],
            vec![2],
            vec![3],
        )?
        .with_authorization_signature(vec![4]);
        let issuer = Arc::new(Issuer {
            peer: sender_id,
            conflict,
            branch,
            member,
            package,
            calls: AtomicUsize::new(0),
            block_first: AtomicBool::new(true),
            block_second: AtomicBool::new(false),
            started: Semaphore::new(0),
            resume: Semaphore::new(0),
            second_started: Semaphore::new(0),
            second_resume: Semaphore::new(0),
        });
        let wire = Arc::new(BranchRecoveryWire);
        let transport = Arc::new(BranchRecoveryTransport::new(sender.clone(), wire.clone()));
        let inbound = Arc::new(BranchRecoveryReceiver::new(
            wire,
            Arc::new(InlineLocalWork),
            issuer.clone(),
        ));
        let serving_manager = receiver.clone();
        let server = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            for index in 0..2 {
                let raw = connections
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("memory inbound closed"))?;
                let manager = serving_manager.clone();
                let inbound = inbound.clone();
                handlers.spawn(async move {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let (connection, stream) = manager.accept_exchange(raw, deadline).await?;
                    let result = inbound.receive(connection, stream, deadline).await;
                    if index == 1 {
                        result?;
                    }
                    anyhow::Ok(())
                });
            }
            while let Some(result) = handlers.join_next().await {
                result??;
            }
            anyhow::Ok(())
        });
        let first = MembershipBranchRecoveryRequest::for_test(
            receiver_id,
            conflict,
            branch,
            member,
            transport.freeze(receiver_id)?,
        );
        let first_transport = transport.clone();
        let waiting = tokio::spawn(async move {
            first_transport
                .request_membership_branch_group_info(first)
                .await
        });
        issuer.started.acquire().await?.forget();
        waiting.abort();
        assert!(matches!(waiting.await, Err(error) if error.is_cancelled()));
        let second = MembershipBranchRecoveryRequest::for_test(
            receiver_id,
            conflict,
            branch,
            member,
            transport.freeze(receiver_id)?,
        );
        let second_result = timeout(
            Duration::from_secs(3),
            transport.request_membership_branch_group_info(second),
        )
        .await;
        // 无论断言如何都释放第一个本地工作，避免测试失败时遗留任务。
        issuer.resume.add_permits(1);
        assert_eq!(second_result??, vec![0x41, 0x42]);
        server.await??;
        assert_eq!(backend.dials.load(Ordering::SeqCst), 2);
        sender.shutdown().await?;
        receiver.shutdown().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn cancelling_inflight_exchange_does_not_cancel_another_peer() -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), async {
        let sender_id = DeviceId::new("memory-recipient");
        let first_peer = DeviceId::new("memory-issuer-one");
        let second_peer = DeviceId::new("memory-issuer-two");
        let allowed = Arc::new(AtomicBool::new(true));
        let (incoming, mut connections) = mpsc::channel(4);
        let backend = Arc::new(MemoryDial {
            target: first_peer,
            second_target: Some(second_peer),
            incoming,
            dials: AtomicUsize::new(0),
        });
        let policy = ConnectivityPolicy {
            outbound_per_peer: 1,
            inbound_per_connection: 1,
            inbound_per_session: 8,
            reject_excess_inbound: true,
            ..ConnectivityPolicy::default()
        };
        let sender = ConnectivityManager::new(
            backend.clone(),
            Arc::new(TwoPeers {
                first: first_peer,
                second: second_peer,
                allowed: allowed.clone(),
            }),
            policy.clone(),
        );
        let receiver = ConnectivityManager::new(
            backend.clone(),
            Arc::new(Members {
                peer: sender_id,
                key: [1; 32],
                allowed,
            }),
            policy,
        );
        let conflict = MembershipConflictId::from_bytes([0x51; 32]);
        let branch = MembershipBranchId::from_bytes([0x52; 32]);
        let member = MemberInstanceId::from_bytes([0x53; 32]);
        let package = MembershipBranchRecoveryPackageV1::new_unsigned(
            conflict,
            branch,
            member,
            member,
            1000,
            [0x54; 32],
            vec![1],
            vec![2],
            vec![3],
        )?
        .with_authorization_signature(vec![4]);
        let issuer = Arc::new(Issuer {
            peer: sender_id,
            conflict,
            branch,
            member,
            package,
            calls: AtomicUsize::new(0),
            block_first: AtomicBool::new(true),
            block_second: AtomicBool::new(true),
            started: Semaphore::new(0),
            resume: Semaphore::new(0),
            second_started: Semaphore::new(0),
            second_resume: Semaphore::new(0),
        });
        let wire = Arc::new(BranchRecoveryWire);
        let transport = Arc::new(BranchRecoveryTransport::new(sender.clone(), wire.clone()));
        let inbound = Arc::new(BranchRecoveryReceiver::new(
            wire,
            Arc::new(InlineLocalWork),
            issuer.clone(),
        ));
        let serving_manager = receiver.clone();
        let server = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            for index in 0..2 {
                let raw = connections
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("memory inbound closed"))?;
                let manager = serving_manager.clone();
                let inbound = inbound.clone();
                handlers.spawn(async move {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let (connection, stream) = manager.accept_exchange(raw, deadline).await?;
                    let result = inbound.receive(connection, stream, deadline).await;
                    if index == 1 {
                        result?;
                    }
                    anyhow::Ok(())
                });
            }
            while let Some(result) = handlers.join_next().await {
                result??;
            }
            anyhow::Ok(())
        });
        let first = MembershipBranchRecoveryRequest::for_test(
            first_peer,
            conflict,
            branch,
            member,
            transport.freeze(first_peer)?,
        );
        let first_transport = transport.clone();
        let first_task = tokio::spawn(async move {
            first_transport
                .request_membership_branch_group_info(first)
                .await
        });
        issuer.started.acquire().await?.forget();
        let second = MembershipBranchRecoveryRequest::for_test(
            second_peer,
            conflict,
            branch,
            member,
            transport.freeze(second_peer)?,
        );
        let second_transport = transport.clone();
        let second_task = tokio::spawn(async move {
            second_transport
                .request_membership_branch_group_info(second)
                .await
        });
        issuer.second_started.acquire().await?.forget();
        first_task.abort();
        assert!(matches!(first_task.await, Err(error) if error.is_cancelled()));
        issuer.second_resume.add_permits(1);
        let second_result = timeout(Duration::from_secs(3), second_task).await;
        issuer.resume.add_permits(1);
        assert_eq!(second_result???, vec![0x41, 0x42]);
        server.await??;
        assert_eq!(backend.dials.load(Ordering::SeqCst), 2);
        sender.shutdown().await?;
        receiver.shutdown().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
