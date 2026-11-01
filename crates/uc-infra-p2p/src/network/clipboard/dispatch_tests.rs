use super::test_support::{dispatch_fixture, EpochOnlyAdmission};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use iroh::EndpointAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uc_application::deps::ConnectivityPolicy;
use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, DispatchAck,
    PeerAddressRepositoryPort, PeerReachabilityPort, SyncPayload,
};
use uc_sync_protocol::clipboard::{self as clipboard_wire, AckCode, CLIPBOARD_ALPN};
const LEGACY_CLIPBOARD_ALPN: &[u8] = b"uniclipboard/clipboard/0";

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, RelayMode};
use tokio::sync::broadcast;

use uc_core::ports::{
    PeerAddressError, PeerAddressRecord, PeerReachabilityChanged, PeerReachabilityError,
    ReachabilityState,
};

// PeerReachabilityPort mock for the dispatch tests. None of the four tests in
// this module reach the dial-failure path (happy ack, duplicate ack,
// missing peer_addr short-circuit, oversized local reject), so the
// adapter never invokes `report_communication_failure`. The mock therefore needs no
// expectations — any accidental call surfaces as a mockall panic, which
// is exactly the regression guard we want for "dispatch tests shouldn't
// be touching peer_reachability state."
//
// `report_communication_failure` is omitted intentionally: it has a default impl on the
// trait (noop), so leaving it off the mock keeps that default in play
// without forcing every test to wire an empty expectation.
mockall::mock! {
    PeerReachability {}

    #[async_trait]
    impl PeerReachabilityPort for PeerReachability {
        async fn ensure_reachable(
            &self,
            device: &DeviceId,
        ) -> Result<ReachabilityState, PeerReachabilityError>;
        async fn current_state(&self, device: &DeviceId) -> ReachabilityState;
        fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged>;
    }
}

fn peer_reachability_mock() -> Arc<dyn PeerReachabilityPort> {
    let mut reachability = MockPeerReachability::new();
    let (events, _) = broadcast::channel(64);
    reachability
        .expect_subscribe()
        .returning(move || events.subscribe());
    Arc::new(reachability)
}

/// In-memory peer_addr_repo the tests use to inject an address blob
/// for a target device. Mirrors the surface the adapter needs without
/// pulling in the Diesel implementation.
#[derive(Default)]
struct MemRepo {
    inner: tokio::sync::Mutex<std::collections::HashMap<String, PeerAddressRecord>>,
}

#[async_trait]
impl PeerAddressRepositoryPort for MemRepo {
    async fn get(&self, device: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(self.inner.lock().await.get(device.as_str()).cloned())
    }
    async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        self.inner
            .lock()
            .await
            .insert(record.device_id.as_str().to_string(), record.clone());
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(self.inner.lock().await.values().cloned().collect())
    }
    async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError> {
        self.inner.lock().await.remove(device.as_str());
        Ok(())
    }
}

async fn bind_endpoint() -> Arc<Endpoint> {
    Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![CLIPBOARD_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .expect("bind endpoint"),
    )
}

async fn wait_for_direct_addrs(endpoint: &Endpoint) {
    for _ in 0..100 {
        if !endpoint.addr().addrs.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("endpoint never published direct addresses");
}

/// Stash an iroh `EndpointAddr` into the mem repo under the given
/// device id, postcard-encoded the same way the pairing wire does.
async fn seed_addr(repo: &MemRepo, device: &DeviceId, addr: &EndpointAddr) {
    let blob = postcard::to_stdvec(addr).expect("postcard encode EndpointAddr");
    repo.upsert(&PeerAddressRecord {
        device_id: device.clone(),
        addr_blob: blob,
        observed_at: Utc::now(),
    })
    .await
    .expect("upsert");
}

fn sample_header() -> ClipboardHeader {
    ClipboardHeader {
        source_entry: None,
        version: ClipboardHeader::CURRENT_VERSION,
        snapshot_hash: "c".repeat(64),
        captured_at_ms: 1_700_000_000_000,
        origin_device_id: "sender-001".to_string(),
        origin_device_name: "Sender".to_string(),
        payload_version: 3,
    }
}

/// Test-only handler that echoes a fixed ack byte after draining the
/// frame. Stored as `Arc<[u8; 1]>` so each spawned task pulls the same
/// configured ack.
#[derive(Clone)]
struct FixedAckHandler {
    ack_byte: u8,
}

impl std::fmt::Debug for FixedAckHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixedAckHandler")
            .field("ack", &format!("0x{:02X}", self.ack_byte))
            .finish()
    }
}

impl ProtocolHandler for FixedAckHandler {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        let ack = self.ack_byte;
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(|err| AcceptError::from_err(err))?;
        // Consume one frame; failure here is fine in negative tests
        // that send bad headers.
        let _ = clipboard_wire::read_frame(&mut recv).await;
        // Emit the configured ack byte regardless.
        send.write_all(&[ack])
            .await
            .map_err(|err| AcceptError::from_err(err))?;
        send.finish().map_err(|err| AcceptError::from_err(err))?;
        let _ = connection.closed().await;
        Ok(())
    }
}

async fn spawn_ack_endpoint(ack: u8) -> (Arc<Endpoint>, Router) {
    let endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&endpoint).await;
    let router = Router::builder((*endpoint).clone())
        .accept(CLIPBOARD_ALPN, FixedAckHandler { ack_byte: ack })
        .spawn();
    (endpoint, router)
}

/// Verdict 1 — happy path. Peer is reachable + accepts the frame.
/// Dispatch returns `DispatchAck::Accepted`.
#[tokio::test]
async fn dispatch_returns_accepted_on_peer_ack_0x01() {
    let (peer_endpoint, peer_router) = spawn_ack_endpoint(AckCode::Accepted.as_byte()).await;
    let peer_addr = peer_endpoint.addr();

    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let repo = Arc::new(MemRepo::default());
    let target = DeviceId::new("target-alpha");
    seed_addr(&repo, &target, &peer_addr).await;

    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );
    let payload = SyncPayload {
        ciphertext: Bytes::from(vec![0x11, 0x22, 0x33, 0x44]),
    };

    let ack = adapter
        .dispatch(&target, &sample_header(), payload)
        .await
        .outcome
        .expect("dispatch succeeds");
    assert_eq!(ack, DispatchAck::Accepted);

    peer_router.shutdown().await.expect("router shutdown");
}

/// Verdict 2 — peer returns the duplicate-ignored ack. Dispatch still
/// returns `Ok`, and the specific variant propagates so the use case
/// can report it distinct from `Accepted`.
#[tokio::test]
async fn dispatch_returns_duplicate_on_peer_ack_0x02() {
    let (peer_endpoint, peer_router) =
        spawn_ack_endpoint(AckCode::DuplicateIgnored.as_byte()).await;
    let peer_addr = peer_endpoint.addr();

    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let repo = Arc::new(MemRepo::default());
    let target = DeviceId::new("target-beta");
    seed_addr(&repo, &target, &peer_addr).await;

    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );
    let payload = SyncPayload {
        ciphertext: Bytes::from(vec![0xAA; 16]),
    };

    let ack = adapter
        .dispatch(&target, &sample_header(), payload)
        .await
        .outcome
        .expect("dispatch succeeds");
    assert_eq!(ack, DispatchAck::DuplicateIgnored);

    peer_router.shutdown().await.expect("router shutdown");
}

#[tokio::test]
async fn dispatch_maps_incompatible_ack_to_the_public_upgrade_error() {
    let (peer_endpoint, peer_router) = spawn_ack_endpoint(AckCode::Incompatible.as_byte()).await;
    let peer_addr = peer_endpoint.addr();
    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let repo = Arc::new(MemRepo::default());
    let target = DeviceId::new("old-engine-target");
    seed_addr(&repo, &target, &peer_addr).await;
    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let outcome = adapter
        .dispatch(
            &target,
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"ciphertext"),
            },
        )
        .await
        .outcome;

    assert!(matches!(
        outcome,
        Err(ClipboardDispatchError::PeerIncompatible)
    ));
    peer_router.shutdown().await.expect("router shutdown");
}

// 流程：B 只监听 0.19 的旧内容入口；A 发送内容时只尝试 1.1 当前入口，不回退或发送旧请求。
#[tokio::test]
async fn dispatch_does_not_fall_back_to_the_v019_clipboard_endpoint() {
    let peer_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&peer_endpoint).await;
    let peer_router = Router::builder((*peer_endpoint).clone())
        .accept(
            LEGACY_CLIPBOARD_ALPN,
            FixedAckHandler {
                ack_byte: AckCode::Accepted.as_byte(),
            },
        )
        .spawn();
    let peer_addr = peer_endpoint.addr();

    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let repo = Arc::new(MemRepo::default());
    let target = DeviceId::new("legacy-target");
    seed_addr(&repo, &target, &peer_addr).await;

    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );
    let result = adapter
        .dispatch(
            &target,
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"must-not-reach-legacy-ingest"),
            },
        )
        .await
        .outcome;

    assert!(matches!(result, Err(ClipboardDispatchError::Offline)));
    peer_router.shutdown().await.expect("router shutdown");
}

/// Verdict 3 — missing peer_addr entry (peer never paired, or entry
/// removed). Dispatch returns `Offline` without touching the network.
#[tokio::test]
async fn dispatch_returns_offline_when_peer_addr_missing() {
    let sender_endpoint = bind_endpoint().await;
    let repo = Arc::new(MemRepo::default());
    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let result = adapter
        .dispatch(
            &DeviceId::new("never-paired"),
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"irrelevant"),
            },
        )
        .await
        .outcome;
    match result {
        Err(ClipboardDispatchError::Offline) => {}
        other => panic!("expected Offline, got {other:?}"),
    }
}

/// Presence fake that counts `report_communication_failure` calls instead of asserting
/// they never happen. Reserved for the single-flight tests below
/// (verdicts 5 and 6) — the happy-path tests above keep using
/// `peer_reachability_mock()` so any accidental call still panics.
struct CountingPeerReachability {
    communication_failure_calls: Arc<std::sync::atomic::AtomicUsize>,
    events: broadcast::Sender<PeerReachabilityChanged>,
}

impl CountingPeerReachability {
    fn new() -> Self {
        let (events, _) = broadcast::channel(8);
        Self {
            communication_failure_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            events,
        }
    }
}

#[async_trait]
impl PeerReachabilityPort for CountingPeerReachability {
    async fn ensure_reachable(
        &self,
        _device: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        Ok(ReachabilityState::Unknown)
    }
    async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
        ReachabilityState::Unknown
    }
    async fn report_communication_failure(&self, _device: &DeviceId) {
        self.communication_failure_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.events.subscribe()
    }
}

/// Verdict 5 — single-flight collapse. Two dispatches racing against
/// the same offline peer must produce exactly one `report_communication_failure` call
/// (the leader's) — followers inherit the verdict through the
/// broadcast and surface their own `Offline` without re-stamping
/// peer_reachability. This is the storm-axis half of #886's acceptance table.
#[tokio::test]
async fn concurrent_dispatch_to_offline_peer_reports_communication_failure_once() {
    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;

    // Seed an unroutable addr blob: a fresh random EndpointId with
    // no transport addrs and relay disabled on the sender, so the
    // staggered retry exhausts its three attempts without ever
    // dialling a live peer.
    let dead_id = iroh::SecretKey::generate().public();
    let dead_addr = EndpointAddr::new(dead_id);

    let target = DeviceId::new("offline-storm-target");
    let repo = Arc::new(MemRepo::default());
    seed_addr(&repo, &target, &dead_addr).await;

    let peer_reachability = Arc::new(CountingPeerReachability::new());
    let communication_failure_calls = Arc::clone(&peer_reachability.communication_failure_calls);

    let adapter = Arc::new(dispatch_fixture(
        sender_endpoint.clone(),
        repo,
        peer_reachability,
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    ));

    let header = sample_header();
    let payload_a = SyncPayload {
        ciphertext: Bytes::from_static(b"first"),
    };
    let payload_b = SyncPayload {
        ciphertext: Bytes::from_static(b"second"),
    };

    let adapter_a = Arc::clone(&adapter);
    let adapter_b = Arc::clone(&adapter);
    let target_a = target.clone();
    let target_b = target.clone();
    let header_a = header.clone();
    let header_b = header.clone();

    let (result_a, result_b) = tokio::join!(
        async move {
            adapter_a
                .dispatch(&target_a, &header_a, payload_a)
                .await
                .outcome
        },
        async move {
            adapter_b
                .dispatch(&target_b, &header_b, payload_b)
                .await
                .outcome
        },
    );

    match (result_a, result_b) {
        (Err(ClipboardDispatchError::Offline), Err(ClipboardDispatchError::Offline)) => {}
        other => panic!("both dispatches should report Offline; got {other:?}"),
    }

    let calls = communication_failure_calls.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        calls, 1,
        "single-flight must collapse report_communication_failure to one call; got {calls}",
    );
}

/// 领头方的 future 在拨号中途被丢弃后，同一目标的后续发送不能永远等一个
/// 不会再有结果的广播，而应重新竞争并给出自己的结果。
#[tokio::test]
async fn dropped_dial_leader_does_not_block_later_dispatches() {
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_open_tx, open) = tokio::sync::watch::channel(true);
    let sender_endpoint = bind_gated_sender(GatedDials {
        started: started.clone(),
        open,
    })
    .await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let dead_addr = EndpointAddr::new(iroh::SecretKey::generate().public());
    let target = DeviceId::new("dropped-leader-target");
    let repo = Arc::new(MemRepo::default());
    seed_addr(&repo, &target, &dead_addr).await;
    let adapter = Arc::new(dispatch_fixture(
        sender_endpoint,
        repo,
        Arc::new(CountingPeerReachability::new()),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    ));

    let leader = {
        let adapter = Arc::clone(&adapter);
        let target = target.clone();
        tokio::spawn(async move {
            adapter
                .dispatch(
                    &target,
                    &sample_header(),
                    SyncPayload {
                        ciphertext: Bytes::from_static(b"leader"),
                    },
                )
                .await
        })
    };
    // 等领头方登记了单飞条目、进入拨号后再丢弃它。
    for _ in 0..200 {
        if started.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        started.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "leader never registered its dial"
    );
    leader.abort();
    let _ = leader.await;

    let later = tokio::time::timeout(
        Duration::from_secs(12),
        adapter.dispatch(
            &target,
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"later"),
            },
        ),
    )
    .await
    .expect("a later dispatch must not wait on a dropped leader");
    assert!(matches!(
        later.outcome,
        Err(ClipboardDispatchError::Offline)
    ));
}

/// 对端收下帧却永不回执时，发送必须在回执上限内以可恢复的暂时失败返回。
#[tokio::test]
async fn dispatch_returns_when_the_peer_never_acks() {
    #[derive(Debug, Clone)]
    struct SilentHandler;
    impl ProtocolHandler for SilentHandler {
        async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
            let (_send, mut recv) = connection
                .accept_bi()
                .await
                .map_err(|err| AcceptError::from_err(err))?;
            let _ = clipboard_wire::read_frame(&mut recv).await;
            let _ = connection.closed().await;
            Ok(())
        }
    }

    let peer_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&peer_endpoint).await;
    let peer_router = Router::builder((*peer_endpoint).clone())
        .accept(CLIPBOARD_ALPN, SilentHandler)
        .spawn();
    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let repo = Arc::new(MemRepo::default());
    let target = DeviceId::new("silent-target");
    seed_addr(&repo, &target, &peer_endpoint.addr()).await;
    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    )
    .with_ack_timeout(Duration::from_millis(500));

    let report = tokio::time::timeout(
        Duration::from_secs(8),
        adapter.dispatch(
            &target,
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"never acked"),
            },
        ),
    )
    .await
    .expect("dispatch must give up when no ack arrives");
    assert!(matches!(
        report.outcome,
        Err(ClipboardDispatchError::Transient(_))
    ));
    peer_router.shutdown().await.expect("router shutdown");
}

/// 对端读完帧后在回执前重置流：确认丢失只是暂时失败，必须可恢复而不是终态 I/O。
#[tokio::test]
async fn stream_reset_before_ack_is_a_transient_dispatch_failure() {
    #[derive(Debug, Clone)]
    struct ResettingHandler;
    impl ProtocolHandler for ResettingHandler {
        async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
            let (mut send, mut recv) = connection
                .accept_bi()
                .await
                .map_err(|err| AcceptError::from_err(err))?;
            let _ = clipboard_wire::read_frame(&mut recv).await;
            let _ = send.reset(0u32.into());
            let _ = connection.closed().await;
            Ok(())
        }
    }

    let peer_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&peer_endpoint).await;
    let peer_router = Router::builder((*peer_endpoint).clone())
        .accept(CLIPBOARD_ALPN, ResettingHandler)
        .spawn();
    let sender_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let repo = Arc::new(MemRepo::default());
    let target = DeviceId::new("resetting-target");
    seed_addr(&repo, &target, &peer_endpoint.addr()).await;
    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let report = tokio::time::timeout(
        Duration::from_secs(8),
        adapter.dispatch(
            &target,
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"reset before ack"),
            },
        ),
    )
    .await
    .expect("dispatch must settle when the stream is reset");
    assert!(
        matches!(report.outcome, Err(ClipboardDispatchError::Transient(_))),
        "unexpected outcome: {:?}",
        report.outcome
    );
    peer_router.shutdown().await.expect("router shutdown");
}

/// 在 `before_connect` 处计数并可暂停拨号，用来观察拨号尝试并在中途撤销成员。
#[derive(Debug, Clone)]
struct GatedDials {
    started: Arc<std::sync::atomic::AtomicUsize>,
    open: tokio::sync::watch::Receiver<bool>,
}

impl iroh::endpoint::EndpointHooks for GatedDials {
    fn before_connect<'a>(
        &'a self,
        _: &'a EndpointAddr,
        _: &'a [u8],
    ) -> impl std::future::Future<Output = iroh::endpoint::BeforeConnectOutcome> + Send + 'a {
        async move {
            self.started
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut open = self.open.clone();
            let _ = open.wait_for(|open| *open).await;
            iroh::endpoint::BeforeConnectOutcome::Accept
        }
    }
}

async fn bind_gated_sender(gate: GatedDials) -> Arc<Endpoint> {
    let endpoint = Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![CLIPBOARD_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .hooks(gate)
            .bind()
            .await
            .expect("bind sender"),
    );
    wait_for_direct_addrs(&endpoint).await;
    endpoint
}

/// 一次发送到不可达对端最多产生 3 次拨号尝试（现有错峰拨号的次数）。
#[tokio::test]
async fn failed_dispatch_makes_at_most_three_dial_attempts() {
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_open_tx, open) = tokio::sync::watch::channel(true);
    let sender = bind_gated_sender(GatedDials {
        started: Arc::clone(&started),
        open,
    })
    .await;
    let dead_addr = EndpointAddr::new(iroh::SecretKey::generate().public());
    let target = DeviceId::new("unreachable-target");
    let repo = Arc::new(MemRepo::default());
    seed_addr(&repo, &target, &dead_addr).await;
    let adapter = dispatch_fixture(
        sender,
        repo,
        Arc::new(CountingPeerReachability::new()),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let report = adapter
        .dispatch(
            &target,
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"unreachable"),
            },
        )
        .await;

    assert!(matches!(
        report.outcome,
        Err(ClipboardDispatchError::Offline)
    ));
    let attempts = started.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        (1..=3).contains(&attempts),
        "one failed send made {attempts} dial attempts"
    );
}

/// 拨号进行到一半时成员被移除：拨号即使随后成功，也不能把内容发给已移除的成员。
#[tokio::test]
async fn dial_finishing_after_member_removal_sends_nothing() {
    dial_revoked_midway_sends_nothing(|connections, target| {
        // 连接 owner 的 `forget` 即调用这一入口。
        connections.close_device(target);
    })
    .await;
}

/// 拨号进行到一半时暂停或离开 Space：同样不能发出内容。
#[tokio::test]
async fn dial_finishing_after_all_connections_closed_sends_nothing() {
    dial_revoked_midway_sends_nothing(|connections, _| {
        // 连接 owner 的 `disconnect_all` 即调用这一入口。
        connections.close_all();
    })
    .await;
}

async fn dial_revoked_midway_sends_nothing(revoke: impl FnOnce(&AdmittedConnections, &DeviceId)) {
    #[derive(Clone)]
    struct CountingAckHandler(Arc<std::sync::atomic::AtomicUsize>);
    impl std::fmt::Debug for CountingAckHandler {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("CountingAckHandler").finish()
        }
    }
    impl ProtocolHandler for CountingAckHandler {
        async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
            let (mut send, mut recv) = connection
                .accept_bi()
                .await
                .map_err(|err| AcceptError::from_err(err))?;
            if clipboard_wire::read_frame(&mut recv).await.is_ok() {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            send.write_all(&[AckCode::Accepted.as_byte()])
                .await
                .map_err(|err| AcceptError::from_err(err))?;
            send.finish().map_err(|err| AcceptError::from_err(err))?;
            let _ = connection.closed().await;
            Ok(())
        }
    }

    let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peer_endpoint = bind_endpoint().await;
    wait_for_direct_addrs(&peer_endpoint).await;
    let peer_router = Router::builder((*peer_endpoint).clone())
        .accept(CLIPBOARD_ALPN, CountingAckHandler(Arc::clone(&frames)))
        .spawn();

    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (open_tx, open) = tokio::sync::watch::channel(false);
    let sender = bind_gated_sender(GatedDials {
        started: Arc::clone(&started),
        open,
    })
    .await;
    let target = DeviceId::new("removed-during-dial");
    let repo = Arc::new(MemRepo::default());
    seed_addr(&repo, &target, &peer_endpoint.addr()).await;
    let connections = AdmittedConnections::default();
    let adapter = Arc::new(dispatch_fixture(
        sender,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(connections.clone())),
        ConnectivityPolicy::default(),
    ));

    let sending = tokio::spawn({
        let adapter = Arc::clone(&adapter);
        let target = target.clone();
        async move {
            adapter
                .dispatch(
                    &target,
                    &sample_header(),
                    SyncPayload {
                        ciphertext: Bytes::from_static(b"must not arrive"),
                    },
                )
                .await
        }
    });
    for _ in 0..200 {
        if started.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        started.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "the dial never started"
    );

    revoke(&connections, &target);
    open_tx.send(true).expect("open the dial gate");

    let report = tokio::time::timeout(Duration::from_secs(10), sending)
        .await
        .expect("dispatch settles")
        .expect("dispatch task joins");
    assert!(report.outcome.is_err(), "a removed member was sent to");
    assert_eq!(
        frames.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the removed member received the frame"
    );
    peer_router.shutdown().await.expect("router shutdown");
}

/// Verdict 4 — oversized payload. The adapter short-circuits before
/// even dialing, returning `LocalPolicyExceeded` with a message that
/// mentions the wire MAX_PAYLOAD_SIZE. Protects against wasted QUIC
/// handshake on a payload that would fail the wire boundary anyway,
/// and uses the dedicated local-policy variant so callers don't
/// misread this as a peer-side rejection.
#[tokio::test]
async fn dispatch_rejects_oversized_payload_locally_without_dialing() {
    // No peer_addr seeded — proves the rejection is local; otherwise
    // we would hit Offline first.
    let sender_endpoint = bind_endpoint().await;
    let repo = Arc::new(MemRepo::default());
    let adapter = dispatch_fixture(
        sender_endpoint,
        repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let oversized = vec![0u8; clipboard_wire::MAX_PAYLOAD_SIZE as usize + 1];
    let result = adapter
        .dispatch(
            &DeviceId::new("irrelevant"),
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from(oversized),
            },
        )
        .await
        .outcome;
    match result {
        Err(ClipboardDispatchError::LocalPolicyExceeded(msg)) => {
            assert!(
                msg.contains("exceeds wire MAX_PAYLOAD_SIZE"),
                "unexpected reject msg: {msg}"
            );
        }
        other => panic!("expected LocalPolicyExceeded, got {other:?}"),
    }
}

/// 地址读取和准入能力异常不能被稳定 Offline 映射吞掉来源。
#[tokio::test]
async fn acquisition_capability_failures_keep_their_source_in_dispatch_reports() {
    use std::error::Error;
    use uc_application::deps::{ConnectivityBackend, ConnectivityError, PeerConnection};
    struct FailingBackend(bool);
    #[async_trait]
    impl ConnectivityBackend for FailingBackend {
        async fn connect(
            &self,
            _: DeviceId,
            _address_hint: Option<Vec<u8>>,
            _cancel: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            let source = Box::new(std::io::Error::other("private-capability-sentinel"));
            Err(if self.0 {
                ConnectivityError::AddressLookup(source)
            } else {
                ConnectivityError::Admission(source)
            })
        }
    }
    for address_lookup in [true, false] {
        let adapter = super::test_support::dispatch_with_backend(
            Arc::new(FailingBackend(address_lookup)),
            Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
            ConnectivityPolicy::default(),
        );
        let error = adapter
            .dispatch(
                &DeviceId::new("peer"),
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"unused"),
                },
            )
            .await
            .outcome
            .unwrap_err();
        assert!(matches!(error, ClipboardDispatchError::Io(_)));
        assert!(!format!("{error}").contains("private-capability-sentinel"));
        let mut current: &(dyn Error + 'static) = &error;
        let mut found = false;
        while let Some(source) = current.source() {
            found |= source.is::<std::io::Error>();
            current = source;
        }
        assert!(
            found,
            "dispatch mapping must retain the original capability failure"
        );
    }
}

/// 取得连接时的准入拒绝（经共享拨号结果返回）是终态授权失败：不能被宽泛的不可用判断
/// 记为可恢复的离线，并保留原始拒绝来源；请求失效与真正的底层拨号失败仍是可恢复的离线。
#[tokio::test]
async fn acquisition_denial_is_a_terminal_failure_while_unreachable_stays_offline() {
    use std::error::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uc_application::deps::{ConnectivityBackend, ConnectivityError, PeerConnection};
    #[derive(Clone, Copy, PartialEq)]
    enum Failure {
        Denied,
        Invalidated,
        Unreachable,
    }
    struct FailingBackend {
        failure: Failure,
        connects: AtomicUsize,
    }
    #[async_trait]
    impl ConnectivityBackend for FailingBackend {
        async fn connect(
            &self,
            _: DeviceId,
            _address_hint: Option<Vec<u8>>,
            _cancel: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            Err(match self.failure {
                Failure::Denied => ConnectivityError::Denied,
                Failure::Invalidated => ConnectivityError::Invalidated(None),
                Failure::Unreachable => ConnectivityError::Backend(Box::new(std::io::Error::from(
                    std::io::ErrorKind::HostUnreachable,
                ))),
            })
        }
    }
    for failure in [Failure::Denied, Failure::Invalidated, Failure::Unreachable] {
        let backend = Arc::new(FailingBackend {
            failure,
            connects: AtomicUsize::new(0),
        });
        let adapter = super::test_support::dispatch_with_backend(
            Arc::clone(&backend) as Arc<dyn ConnectivityBackend>,
            Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
            ConnectivityPolicy::default(),
        );
        let error = adapter
            .dispatch(
                &DeviceId::new("peer"),
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"unused"),
                },
            )
            .await
            .outcome
            .unwrap_err();
        assert_eq!(backend.connects.load(Ordering::SeqCst), 1);
        if failure != Failure::Denied {
            assert!(matches!(error, ClipboardDispatchError::Offline), "{error}");
            continue;
        }
        assert!(matches!(error, ClipboardDispatchError::Io(_)), "{error}");
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<ConnectivityError>());
        assert!(
            source.is_some_and(ConnectivityError::is_denied),
            "dispatch mapping must retain the typed denial"
        );
    }
}

/// 经真实连接准入：拨号进行中撤销快照失效（锁定准备暂停全部连接）而成员关系不变时，取得
/// 连接得到可恢复的离线结果；旧请求仍被围栏拒绝，迟到的连接被关闭且从未打开流或发送帧。
/// 成员权威真实拒绝与身份不符仍是保留类型拒绝来源的终态失败。
#[tokio::test]
async fn acquisition_invalidated_during_dial_is_recoverable_while_real_refusal_is_terminal() {
    use crate::network::iroh::connectivity::{ConnectivityProfile, IrohConnectionAdmission};
    use std::error::Error;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::{watch, Notify};
    use uc_application::deps::{
        ConnectionMode, ConnectionProtocol, ConnectivityBackend, ConnectivityError,
        KnownPeerIdentity, MembershipLedgerError, PeerConnection, PeerIdentityDirectoryPort,
        StreamPair,
    };
    use uc_core::membership::{PeerAdmissionError, PeerAdmissionPort};
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::ConnectionChannel;
    use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

    const PEER_KEY: [u8; 32] = [0x11; 32];
    const OTHER_KEY: [u8; 32] = [0x22; 32];
    struct Members {
        identities: Vec<KnownPeerIdentity>,
        admitted: AtomicBool,
    }
    #[async_trait]
    impl PeerIdentityDirectoryPort for Members {
        async fn known_peer_identities(
            &self,
        ) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
            Ok(self.identities.clone())
        }
    }
    #[async_trait]
    impl PeerAdmissionPort for Members {
        async fn is_admitted(&self, peer: &DeviceId) -> Result<bool, PeerAdmissionError> {
            Ok(self.admitted.load(Ordering::SeqCst)
                && self.identities.iter().any(|row| row.device_id == *peer))
        }
    }
    struct LateConnection {
        key: [u8; 32],
        opened: AtomicUsize,
        closed: watch::Sender<bool>,
    }
    #[async_trait]
    impl PeerConnection for LateConnection {
        fn id(&self) -> usize {
            1
        }
        fn authenticated_key(&self) -> [u8; 32] {
            self.key
        }
        fn mode(&self) -> ConnectionMode {
            ConnectionMode::Multiplexed
        }
        fn protocol(&self) -> ConnectionProtocol {
            ConnectionProtocol::MultiplexedClipboard
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
            let mut closed = self.closed.subscribe();
            let _ = closed.wait_for(|closed| *closed).await;
        }
        async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            Err(ConnectivityError::Offline)
        }
        async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
            Err(ConnectivityError::Stopped)
        }
    }
    /// 拨号停在门口，直到测试完成失效或放行。
    struct GatedBackend {
        connection: Arc<LateConnection>,
        dialing: Notify,
        release: Notify,
    }
    #[async_trait]
    impl ConnectivityBackend for GatedBackend {
        async fn connect(
            &self,
            _: DeviceId,
            _address_hint: Option<Vec<u8>>,
            _cancel: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            self.dialing.notify_one();
            self.release.notified().await;
            Ok(self.connection.clone())
        }
    }
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Case {
        PausedWhileAdmitted,
        MembershipRefused,
        IdentityMismatch,
    }
    let fingerprint = Sha256IdentityFingerprintFactory;
    let peer = DeviceId::new("peer");
    for case in [
        Case::PausedWhileAdmitted,
        Case::MembershipRefused,
        Case::IdentityMismatch,
    ] {
        let members = Arc::new(Members {
            identities: vec![
                KnownPeerIdentity {
                    device_id: peer,
                    identity_fingerprint: fingerprint.from_public_key(&PEER_KEY).unwrap(),
                },
                KnownPeerIdentity {
                    device_id: DeviceId::new("other"),
                    identity_fingerprint: fingerprint.from_public_key(&OTHER_KEY).unwrap(),
                },
            ],
            admitted: AtomicBool::new(case != Case::MembershipRefused),
        });
        let registry = AdmittedConnections::default();
        let backend = Arc::new(GatedBackend {
            connection: Arc::new(LateConnection {
                key: if case == Case::IdentityMismatch {
                    OTHER_KEY
                } else {
                    PEER_KEY
                },
                opened: AtomicUsize::new(0),
                closed: watch::channel(false).0,
            }),
            dialing: Notify::new(),
            release: Notify::new(),
        });
        let adapter = super::test_support::dispatch_with_backend(
            Arc::clone(&backend) as Arc<dyn ConnectivityBackend>,
            Arc::new(IrohConnectionAdmission::new(
                members.clone(),
                members,
                Arc::new(Sha256IdentityFingerprintFactory),
                registry.clone(),
                ConnectivityProfile::Clipboard,
            )),
            ConnectivityPolicy::default(),
        );
        let dispatch = tokio::spawn(async move {
            adapter
                .dispatch(
                    &peer,
                    &sample_header(),
                    SyncPayload {
                        ciphertext: Bytes::from_static(b"payload"),
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), backend.dialing.notified())
            .await
            .expect("dial reaches the gate");
        if case == Case::PausedWhileAdmitted {
            // 与锁定准备暂停连接相同：推进撤销快照，成员关系保持准入。
            registry.close_all(b"test pause");
        }
        backend.release.notify_one();
        let report = tokio::time::timeout(Duration::from_secs(5), dispatch)
            .await
            .expect("dispatch settles")
            .expect("dispatch task");
        let error = report.outcome.unwrap_err();
        assert_eq!(
            backend.connection.opened.load(Ordering::SeqCst),
            0,
            "{case:?}: a rejected acquisition must not open a stream"
        );
        assert!(
            backend.connection.is_closed(),
            "{case:?}: the late connection must be closed"
        );
        if case == Case::PausedWhileAdmitted {
            assert!(
                matches!(error, ClipboardDispatchError::Offline),
                "{case:?}: {error}"
            );
            continue;
        }
        assert!(
            matches!(error, ClipboardDispatchError::Io(_)),
            "{case:?}: {error}"
        );
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<ConnectivityError>());
        assert!(
            source.is_some_and(ConnectivityError::is_denied),
            "{case:?}: dispatch mapping must retain the typed denial"
        );
    }
}

#[path = "source_identity_tests.rs"]
mod source_identity_tests;
