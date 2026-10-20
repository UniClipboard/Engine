//! 真实 Iroh + 完整 Application owner；不查询或改写业务观察表。
use super::super::admitted_connections::AdmittedConnections;
use super::super::connectivity::{
    ConnectivityProfile, IrohConnectionAdmission, IrohConnectivityBackend, PeerReachabilityWire,
    PresenceEnvironment,
};
use super::super::net_recovery::{DemandRecoveryCoordinator, NetworkRecoveryObservationSource};
use super::{IrohPeerReachabilityHandler, LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN};
use async_trait::async_trait;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::broadcast;
use uc_application::deps::{
    ConnectivityManager, ConnectivityPolicy, KnownPeerIdentity, MembershipLedgerError,
    PeerIdentityDirectoryPort, PeerReachabilityTransport, PeerReachabilityTransportPorts,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{
    PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort, PeerReachabilityPort,
    ReachabilityState,
};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
use uc_infra_local::SystemClock;

#[derive(Default)]
struct Addresses(Mutex<HashMap<DeviceId, PeerAddressRecord>>);
#[async_trait]
impl PeerAddressRepositoryPort for Addresses {
    async fn get(&self, peer: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(self.0.lock().unwrap().get(peer).cloned())
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }
    async fn upsert(&self, value: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        self.0
            .lock()
            .unwrap()
            .insert(value.device_id, value.clone());
        Ok(())
    }
    async fn remove(&self, peer: &DeviceId) -> Result<(), PeerAddressError> {
        self.0.lock().unwrap().remove(peer);
        Ok(())
    }
}
struct Directory(Vec<KnownPeerIdentity>);
#[async_trait]
impl PeerIdentityDirectoryPort for Directory {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(self.0.clone())
    }
}
#[derive(Default)]
struct Authority {
    denied: AtomicBool,
    failed: AtomicBool,
    calls: AtomicUsize,
    failure_on: AtomicUsize,
}
#[async_trait]
impl PeerAdmissionPort for Authority {
    async fn is_admitted(&self, _: &DeviceId) -> Result<bool, PeerAdmissionError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.failed.load(Ordering::SeqCst) || self.failure_on.load(Ordering::SeqCst) == call {
            return Err(PeerAdmissionError::ReadFailed {
                source: Box::new(std::io::Error::other("private-source-sentinel")),
            });
        }
        Ok(!self.denied.load(Ordering::SeqCst))
    }
}
async fn endpoint() -> Arc<Endpoint> {
    Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .clear_address_lookup()
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![
                PEER_REACHABILITY_ALPN.to_vec(),
                LEGACY_PEER_REACHABILITY_ALPN.to_vec(),
            ])
            .bind()
            .await
            .unwrap(),
    )
}
fn identities(peer: &Endpoint, name: &str) -> Arc<Directory> {
    Arc::new(Directory(vec![KnownPeerIdentity {
        device_id: DeviceId::new(name),
        identity_fingerprint: Sha256IdentityFingerprintFactory
            .from_public_key(peer.id().as_bytes())
            .unwrap(),
    }]))
}
fn owner(
    endpoint: Arc<Endpoint>,
    addresses: Arc<Addresses>,
    directory: Arc<Directory>,
    authority: Arc<Authority>,
) -> Arc<PeerReachabilityTransport> {
    let fingerprint = Arc::new(Sha256IdentityFingerprintFactory);
    let connections = AdmittedConnections::default();
    let manager = ConnectivityManager::new(
        Arc::new(IrohConnectivityBackend::new(
            endpoint.clone(),
            addresses,
            None,
            ConnectivityProfile::Presence,
        )),
        Arc::new(IrohConnectionAdmission::new(
            directory.clone(),
            authority.clone(),
            fingerprint.clone(),
            connections.clone(),
            ConnectivityProfile::Presence,
        )),
        ConnectivityPolicy {
            presence_retention: true,
            inbound_per_connection: 2,
            flights_per_session: 4,
            ..ConnectivityPolicy::default()
        },
    );
    PeerReachabilityTransport::new(
        manager,
        authority,
        PeerReachabilityTransportPorts {
            wire: Arc::new(PeerReachabilityWire),
            environment: Arc::new(PresenceEnvironment::new(
                directory,
                fingerprint,
                connections,
                Arc::new(DemandRecoveryCoordinator::new((*endpoint).clone(), false)),
                Arc::new(NetworkRecoveryObservationSource::new()),
            )),
        },
        Arc::new(SystemClock),
        broadcast::channel(64).0,
    )
}
async fn wait_online(owner: &PeerReachabilityTransport, peer: &DeviceId) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while owner.current_state(peer).await != ReachabilityState::Online {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn admission(raw: &iroh::endpoint::Connection) -> u8 {
    let (mut send, mut receive) = raw.open_bi().await.unwrap();
    send.write_all(&[1]).await.unwrap();
    send.finish().unwrap();
    receive.read_to_end(1).await.unwrap()[0]
}
#[tokio::test]
async fn seventeenth_connection_without_a_first_stream_is_closed_immediately() {
    let server = endpoint().await;
    let client = endpoint().await;
    let owner = owner(
        server.clone(),
        Arc::new(Addresses::default()),
        identities(&client, "client"),
        Arc::new(Authority::default()),
    );
    let router = Router::builder((*server).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(owner.clone()),
        )
        .spawn();
    let mut raw = Vec::new();
    for _ in 0..17 {
        raw.push(
            client
                .connect(server.addr(), PEER_REACHABILITY_ALPN)
                .await
                .unwrap(),
        );
    }
    tokio::time::timeout(Duration::from_secs(1), raw[16].closed())
        .await
        .unwrap();
    owner.disconnect_all().await;
    for connection in &raw {
        tokio::time::timeout(Duration::from_secs(2), connection.closed())
            .await
            .unwrap();
    }
    router.shutdown().await.unwrap();
    client.close().await;
}
#[tokio::test]
async fn third_admitted_inbound_replaces_one_previous_after_success_and_keeps_online() {
    let server = endpoint().await;
    let client = endpoint().await;
    let owner = owner(
        server.clone(),
        Arc::new(Addresses::default()),
        identities(&client, "client"),
        Arc::new(Authority::default()),
    );
    let router = Router::builder((*server).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(owner.clone()),
        )
        .spawn();
    let mut raw = Vec::new();
    for _ in 0..3 {
        let connection = client
            .connect(server.addr(), PEER_REACHABILITY_ALPN)
            .await
            .unwrap();
        assert_eq!(admission(&connection).await, 1);
        raw.push(connection);
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while raw[..2]
            .iter()
            .all(|connection| connection.close_reason().is_none())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        raw[..2]
            .iter()
            .filter(|connection| connection.close_reason().is_some())
            .count(),
        1
    );
    assert!(raw[0].close_reason().is_some());
    assert!(raw[1].close_reason().is_none());
    assert!(raw[2].close_reason().is_none());
    assert_eq!(
        owner.current_state(&DeviceId::new("client")).await,
        ReachabilityState::Online
    );
    owner.disconnect_all().await;
    router.shutdown().await.unwrap();
    client.close().await;
}
#[tokio::test]
async fn both_directions_can_verify_the_same_v2_connection_without_redial_or_address() {
    let a = endpoint().await;
    let b = endpoint().await;
    let addresses = Arc::new(Addresses::default());
    addresses
        .upsert(&PeerAddressRecord {
            device_id: DeviceId::new("b"),
            addr_blob: postcard::to_stdvec(&b.addr()).unwrap(),
            observed_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let left = owner(
        a.clone(),
        addresses.clone(),
        identities(&b, "b"),
        Arc::new(Authority::default()),
    );
    let right = owner(
        b.clone(),
        Arc::new(Addresses::default()),
        identities(&a, "a"),
        Arc::new(Authority::default()),
    );
    let router = Router::builder((*b).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(right.clone()),
        )
        .spawn();
    assert_eq!(
        left.verify_reachable(&DeviceId::new("b")).await.unwrap(),
        ReachabilityState::Online
    );
    wait_online(&right, &DeviceId::new("a")).await;
    addresses.remove(&DeviceId::new("b")).await.unwrap();
    let a_id = DeviceId::new("a");
    let b_id = DeviceId::new("b");
    for _ in 0..3 {
        let (one, two) = tokio::join!(left.verify_reachable(&b_id), right.verify_reachable(&a_id));
        assert_eq!(one.unwrap(), ReachabilityState::Online);
        assert_eq!(two.unwrap(), ReachabilityState::Online);
    }
    left.disconnect_all().await;
    right.disconnect_all().await;
    router.shutdown().await.unwrap();
    a.close().await;
}
#[tokio::test]
async fn local_authority_failure_ends_only_the_liveness_stream_without_rejection_or_connection_close(
) {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _logs = logs.install();
    let server = endpoint().await;
    let client = endpoint().await;
    let authority = Arc::new(Authority::default());
    let owner = owner(
        server.clone(),
        Arc::new(Addresses::default()),
        identities(&client, "client"),
        authority.clone(),
    );
    let router = Router::builder((*server).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(owner.clone()),
        )
        .spawn();
    let connection = client
        .connect(server.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    assert_eq!(admission(&connection).await, 1);
    wait_online(&owner, &DeviceId::new("client")).await;
    authority.failed.store(true, Ordering::SeqCst);
    let (mut send, mut receive) = connection.open_bi().await.unwrap();
    send.write_all(&[0x10; 17]).await.unwrap();
    send.finish().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), receive.read_to_end(17))
        .await
        .unwrap();
    assert!(
        !matches!(result, Ok(ref bytes) if bytes.first() == Some(&0x12)),
        "a local failure is not a peer rejection"
    );
    assert!(connection.close_reason().is_none());
    assert_eq!(
        owner.current_state(&DeviceId::new("client")).await,
        ReachabilityState::Online
    );
    connection.close(0u32.into(), b"PRIVATE_NATIVE_REASON");
    tokio::time::timeout(Duration::from_secs(2), async {
        while logs.count("presence.connection.closed") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(logs.count("presence.connection.closed"), 1);
    // CapturedLogs 包含第三方 TRACE 原文；本例验证受控记录，完整导出过滤由 runtime 验收。
    assert!(logs
        .output()
        .lines()
        .filter(|line| line.contains("uc.connectivity"))
        .all(|line| !line.contains("PRIVATE_NATIVE_REASON")));
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(owner.current_state(&DeviceId::new("client")).await, ReachabilityState::Online, "a close following the local fault must not replace the retained display with fabricated Offline");
    owner.disconnect_all().await;
    router.shutdown().await.unwrap();
    client.close().await;
}

#[tokio::test]
async fn genuine_rejection_wins_over_an_alive_parallel_connection_without_a_new_dial() {
    let server = endpoint().await;
    let client = endpoint().await;
    let owner = owner(
        server.clone(),
        Arc::new(Addresses::default()),
        identities(&client, "client"),
        Arc::new(Authority::default()),
    );
    let router = Router::builder((*server).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(owner.clone()),
        )
        .spawn();
    let one = client
        .connect(server.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    let two = client
        .connect(server.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    assert_eq!(admission(&one).await, 1);
    assert_eq!(admission(&two).await, 1);
    wait_online(&owner, &DeviceId::new("client")).await;
    let _retained_connections = (one.clone(), two.clone());
    let first = tokio::spawn(async move {
        let (mut send, mut receive) = one.accept_bi().await.unwrap();
        let frame = receive.read_to_end(17).await.unwrap();
        let nonce = uc_sync_protocol::peer_reachability::challenge(&frame).unwrap();
        send.write_all(&uc_sync_protocol::peer_reachability::reply(&nonce, true))
            .await
            .unwrap();
        send.finish().unwrap();
    });
    let second = tokio::spawn(async move {
        let (mut send, mut receive) = two.accept_bi().await.unwrap();
        let frame = receive.read_to_end(17).await.unwrap();
        let nonce = uc_sync_protocol::peer_reachability::challenge(&frame).unwrap();
        send.write_all(&uc_sync_protocol::peer_reachability::reply(&nonce, false))
            .await
            .unwrap();
        send.finish().unwrap();
    });
    assert_eq!(
        owner
            .verify_reachable(&DeviceId::new("client"))
            .await
            .unwrap(),
        ReachabilityState::Offline
    );
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(
        owner.current_state(&DeviceId::new("client")).await,
        ReachabilityState::Offline
    );
    owner.disconnect_all().await;
    router.shutdown().await.unwrap();
    client.close().await;
}

/// 已验证的远端拒绝优先于同轮另一连接的本机读取故障；故障本身不会伪装成拒绝。
#[tokio::test]
async fn genuine_remote_rejection_wins_over_a_parallel_local_grant_read_failure() {
    let server = endpoint().await;
    let client = endpoint().await;
    let authority = Arc::new(Authority::default());
    let owner = owner(
        server.clone(),
        Arc::new(Addresses::default()),
        identities(&client, "client"),
        authority.clone(),
    );
    let router = Router::builder((*server).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(owner.clone()),
        )
        .spawn();
    let one = client
        .connect(server.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    let two = client
        .connect(server.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    assert_eq!(admission(&one).await, 1);
    assert_eq!(admission(&two).await, 1);
    wait_online(&owner, &DeviceId::new("client")).await;
    authority
        .failure_on
        .store(authority.calls.load(Ordering::SeqCst) + 2, Ordering::SeqCst);
    let verification = tokio::spawn({
        let owner = owner.clone();
        async move { owner.verify_reachable(&DeviceId::new("client")).await }
    });
    // 只有通过本机 grant 的候选实际开流；另一条的本机故障没有生成任何回复。
    let (mut send, mut receive) = tokio::select! { result = one.accept_bi() => result.unwrap(), result = two.accept_bi() => result.unwrap() };
    let frame = receive.read_to_end(17).await.unwrap();
    let nonce = uc_sync_protocol::peer_reachability::challenge(&frame).unwrap();
    send.write_all(&uc_sync_protocol::peer_reachability::reply(&nonce, false))
        .await
        .unwrap();
    send.finish().unwrap();
    assert_eq!(
        verification.await.unwrap().unwrap(),
        ReachabilityState::Offline
    );
    assert_eq!(
        owner.current_state(&DeviceId::new("client")).await,
        ReachabilityState::Offline
    );
    owner.shutdown().await.unwrap();
    router.shutdown().await.unwrap();
    client.close().await;
}

/// 两个缺 FIN 的请求占用两份活动；超量只结束该流，后续请求仍能在原连接上完成。
#[tokio::test]
async fn third_liveness_stream_is_rejected_without_closing_the_connection_and_capacity_returns() {
    let server = endpoint().await;
    let client = endpoint().await;
    let owner = owner(
        server.clone(),
        Arc::new(Addresses::default()),
        identities(&client, "client"),
        Arc::new(Authority::default()),
    );
    let router = Router::builder((*server).clone())
        .accept(
            PEER_REACHABILITY_ALPN,
            IrohPeerReachabilityHandler::new(owner.clone()),
        )
        .spawn();
    let raw = client
        .connect(server.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    assert_eq!(admission(&raw).await, 1);
    wait_online(&owner, &DeviceId::new("client")).await;
    let mut pending = Vec::new();
    for _ in 0..2 {
        let (mut send, receive) = raw.open_bi().await.unwrap();
        send.write_all(&[0x10]).await.unwrap();
        pending.push((send, receive));
    }
    // 数据已发送后让真实服务端取出并占用两份活动。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let nonce = [0xAB; 16];
    let (mut excess, mut denied) = raw.open_bi().await.unwrap();
    excess
        .write_all(&uc_sync_protocol::peer_reachability::request(&nonce))
        .await
        .unwrap();
    excess.finish().unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(1), denied.read_to_end(17))
        .await
        .unwrap();
    assert!(!matches!(reply, Ok(ref bytes) if bytes.first() == Some(&0x11)));
    assert!(raw.close_reason().is_none());
    for (mut send, mut receive) in pending {
        send.write_all(&nonce).await.unwrap();
        send.finish().unwrap();
        let bytes = receive.read_to_end(17).await.unwrap();
        assert_eq!(
            uc_sync_protocol::peer_reachability::classify(&bytes, &nonce).unwrap(),
            uc_sync_protocol::peer_reachability::CheckReply::Accepted
        );
    }
    let (mut send, mut receive) = raw.open_bi().await.unwrap();
    send.write_all(&uc_sync_protocol::peer_reachability::request(&nonce))
        .await
        .unwrap();
    send.finish().unwrap();
    let bytes = receive.read_to_end(17).await.unwrap();
    assert_eq!(
        uc_sync_protocol::peer_reachability::classify(&bytes, &nonce).unwrap(),
        uc_sync_protocol::peer_reachability::CheckReply::Accepted
    );
    assert_eq!(
        owner.current_state(&DeviceId::new("client")).await,
        ReachabilityState::Online
    );
    owner.shutdown().await.unwrap();
    router.shutdown().await.unwrap();
    client.close().await;
}
