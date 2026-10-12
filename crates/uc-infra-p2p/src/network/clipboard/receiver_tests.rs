use crate::network::iroh::admitted_connections::AdmittedConnections;
use crate::network::iroh::connectivity::ConnectivityProfile;
use crate::network::iroh::connectivity::{IrohClipboardHandler, IrohConnectivityBackend};
use crate::network::iroh::inbound_peer::{member_table_directory, member_table_identity_directory};
use crate::network::iroh::StaticPeerAdmission;
use tokio::sync::broadcast;
use tracing::Instrument;
use uc_application::deps::ConnectivityPolicy;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectivityBackend, ConnectivityError,
    PeerConnection, SessionConnectionAdmissionPort,
};
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationContext,
};

use super::test_support::{dispatch_fixture, EpochOnlyAdmission, ReceiverFixture};
use uc_application::deps::ClipboardDelivery;
use uc_core::ids::DeviceId;
use uc_core::membership::MemberRepositoryPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_sync_protocol::clipboard::{
    self as clipboard_wire, AckCode, CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN,
};

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use futures_util::StreamExt;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use opentelemetry::trace::TraceContextExt;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tokio::sync::{Mutex, Notify, Semaphore};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use uc_core::membership::{MembershipError, SpaceMember};
use uc_core::ports::{
    ClipboardDispatchPort, ClipboardHeader, DispatchAck, InboundClipboardDisposition,
    PeerReachabilityChanged, PeerReachabilityError, PeerReachabilityPort, ReachabilityState,
    SyncPayload,
};
use uc_core::MemberSyncPreferences;

use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

struct TestTelemetry {
    traces: SdkTracerProvider,
    logs: SdkLoggerProvider,
    span_exporter: InMemorySpanExporter,
    log_exporter: InMemoryLogExporter,
}

fn test_telemetry() -> &'static TestTelemetry {
    static TELEMETRY: OnceLock<TestTelemetry> = OnceLock::new();
    TELEMETRY.get_or_init(|| {
        let span_exporter = InMemorySpanExporter::default();
        let log_exporter = InMemoryLogExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .build();
        let trace_layer = tracing_opentelemetry::layer()
            .with_tracer(traces.tracer("clipboard-trace-test"))
            .with_context_activation(true)
            .with_filter(filter_fn(|metadata| {
                metadata.is_span()
                    && metadata.target() == uc_observability_contract::diagnostics::TELEMETRY_TARGET
            }));
        let log_layer = OpenTelemetryTracingBridge::new(&logs).with_filter(filter_fn(|metadata| {
            metadata.is_event()
                && metadata.target() == uc_observability_contract::diagnostics::TELEMETRY_TARGET
        }));
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry()
                .with(trace_layer)
                .with(log_layer),
        )
        .expect("install clipboard trace test subscriber");
        TestTelemetry {
            traces,
            logs,
            span_exporter,
            log_exporter,
        }
    })
}

// PeerReachabilityPort mock. Receiver-side tests use the dispatch adapter only
// as a wire-driver against the receiver under test; the dispatch path
// never hits dial failure in these scenarios, so `report_communication_failure` is
// never invoked. The mock therefore needs no expectations — any
// unexpected call surfaces as a mockall panic.
//
// `report_communication_failure` is intentionally omitted from the mock so the trait's
// default noop impl is in play; if a future test does exercise the
// dial-failure path, mockall will catch any accidental peer_reachability-side
// assumption it makes.
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

// ----- test doubles ------------------------------------------------------

#[derive(Default)]
struct MemMemberRepo {
    inner: Mutex<HashMap<String, SpaceMember>>,
}
member_table_identity_directory!(MemMemberRepo);
#[async_trait]
impl MemberRepositoryPort for MemMemberRepo {
    async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(self.inner.lock().await.get(device_id.as_str()).cloned())
    }
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        Ok(self.inner.lock().await.values().cloned().collect())
    }
    async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
        self.inner
            .lock()
            .await
            .insert(member.device_id.as_str().to_string(), member.clone());
        Ok(())
    }
    async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
        Ok(self.inner.lock().await.remove(device_id.as_str()).is_some())
    }
}

#[derive(Default)]
struct MemPeerAddrRepo {
    inner: Mutex<HashMap<String, PeerAddressRecord>>,
}
#[async_trait]
impl PeerAddressRepositoryPort for MemPeerAddrRepo {
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

// ----- harness -----------------------------------------------------------

async fn bind_endpoint_with(seed: [u8; 32]) -> Arc<Endpoint> {
    Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(SecretKey::from_bytes(&seed))
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

fn make_member(seed: [u8; 32], device_id: &str) -> SpaceMember {
    let factory = Sha256IdentityFingerprintFactory;
    let sk = SecretKey::from_bytes(&seed);
    let fp = factory
        .from_public_key(sk.public().as_bytes())
        .expect("derive fingerprint for test member");
    SpaceMember {
        device_id: DeviceId::new(device_id),
        device_name: "Test Device".to_string(),
        identity_fingerprint: fp,
        joined_at: Utc::now(),
        sync_preferences: MemberSyncPreferences::default(),
    }
}

fn sample_header() -> ClipboardHeader {
    ClipboardHeader {
        version: ClipboardHeader::CURRENT_VERSION,
        snapshot_hash: "9".repeat(64),
        captured_at_ms: 1_700_000_000_000,
        origin_device_id: "sender-001".to_string(),
        origin_device_name: "Alice".to_string(),
        payload_version: 3,
    }
}

/// Spin up a receiver router around a fresh adapter and return the
/// handles the test needs: the router (to shut down), the receiver's
/// endpoint addr (for the dialer to connect to), and a subscriber to
/// the inbound stream.
struct ReceiverHarness {
    receiver_endpoint: Arc<Endpoint>,
    receiver_router: Router,
    inbound_rx: broadcast::Receiver<ClipboardDelivery>,
}

async fn spawn_receiver(
    seed: [u8; 32],
    member_repo: Arc<dyn MemberRepositoryPort>,
) -> ReceiverHarness {
    spawn_receiver_with_admission(seed, member_repo, true).await
}

async fn spawn_receiver_with_admission(
    seed: [u8; 32],
    member_repo: Arc<dyn MemberRepositoryPort>,
    admitted: bool,
) -> ReceiverHarness {
    let receiver_endpoint = bind_endpoint_with(seed).await;
    wait_for_direct_addrs(&receiver_endpoint).await;

    let adapter = ReceiverFixture::new(
        member_table_directory(member_repo),
        Arc::new(StaticPeerAdmission(admitted)),
        Arc::new(Sha256IdentityFingerprintFactory),
    );
    let inbound_rx = adapter.subscribe();
    let router = Router::builder((*receiver_endpoint).clone())
        .accept(CLIPBOARD_ALPN, adapter.handler())
        .accept(MULTIPLEXED_CLIPBOARD_ALPN, adapter.handler())
        .spawn();

    router.endpoint().set_alpns(vec![
        MULTIPLEXED_CLIPBOARD_ALPN.to_vec(),
        CLIPBOARD_ALPN.to_vec(),
    ]);
    ReceiverHarness {
        receiver_endpoint,
        receiver_router: router,
        inbound_rx,
    }
}

/// 同一条真实连接的第二条流不应等待第一份内容的应用结算。
#[tokio::test]
async fn receives_two_streams_before_first_receipt_settles() {
    let telemetry = test_telemetry();
    let contexts = [
        uc_sync_protocol::WireTraceContext {
            traceparent: "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-1111111111111111-01".into(),
        },
        uc_sync_protocol::WireTraceContext {
            traceparent: "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2-2222222222222222-01".into(),
        },
    ];
    let sender_seed = [0x31; 32];
    let members: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    members
        .save(&make_member(sender_seed, "sender"))
        .await
        .unwrap();
    let mut harness = spawn_receiver([0x32; 32], members).await;
    let sender = bind_endpoint_with(sender_seed).await;
    let connection = sender
        .connect(harness.receiver_endpoint.addr(), MULTIPLEXED_CLIPBOARD_ALPN)
        .await
        .unwrap();
    let mut replies = Vec::new();
    for (payload, context) in [b"first".as_slice(), b"second".as_slice()]
        .into_iter()
        .zip(contexts.iter())
    {
        let (mut send, recv) = connection.open_bi().await.unwrap();
        clipboard_wire::write_frame(
            &mut send,
            &sample_header(),
            Some(context.clone()),
            &Bytes::copy_from_slice(payload),
        )
        .await
        .unwrap();
        send.finish().unwrap();
        replies.push(recv);
    }
    let first = tokio::time::timeout(Duration::from_secs(3), harness.inbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(1), harness.inbound_rx.recv())
        .await
        .expect("second stream must arrive before the first receipt settles")
        .unwrap();
    assert_eq!(first.message.ciphertext.as_ref(), b"first");
    assert_eq!(second.message.ciphertext.as_ref(), b"second");
    assert!(second
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied));
    let mut ack = [0];
    tokio::time::timeout(Duration::from_secs(3), replies[1].read_exact(&mut ack))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack, [AckCode::Accepted.as_byte()]);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), replies[0].read_exact(&mut ack))
            .await
            .is_err()
    );
    assert!(first
        .message
        .receipt
        .finish(InboundClipboardDisposition::Duplicate));
    tokio::time::timeout(Duration::from_secs(3), replies[0].read_exact(&mut ack))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack, [AckCode::DuplicateIgnored.as_byte()]);
    connection.close(0u32.into(), b"test_complete");
    harness.receiver_router.shutdown().await.unwrap();
    sender.close().await;
    telemetry.traces.force_flush().unwrap();
    let spans = telemetry.span_exporter.get_finished_spans().unwrap();
    for (trace_id, parent_id) in [
        ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1", "1111111111111111"),
        ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2", "2222222222222222"),
    ] {
        assert!(
            spans
                .iter()
                .any(|span| span.span_context.trace_id().to_string() == trace_id
                    && span.parent_span_id.to_string() == parent_id
                    && span
                        .attributes
                        .iter()
                        .any(|attr| attr.key.as_str() == "uc.operation"
                            && attr.value.as_str() == "clipboard_receive")),
            "each stream must preserve its own remote parent"
        );
    }
}

/// 错误帧只拒绝本流；同连接后续正常帧仍能持久接受。
#[tokio::test]
async fn malformed_stream_does_not_close_the_multiplexed_connection() {
    let seed = [0x41; 32];
    let sender = bind_endpoint_with(seed).await;
    let members = Arc::new(MemMemberRepo::default());
    members.save(&make_member(seed, "known")).await.unwrap();
    let mut harness = spawn_receiver([0x42; 32], members).await;
    let connection = sender
        .connect(harness.receiver_endpoint.addr(), MULTIPLEXED_CLIPBOARD_ALPN)
        .await
        .unwrap();
    assert_eq!(connection.alpn(), MULTIPLEXED_CLIPBOARD_ALPN);
    let (mut bad_send, mut bad_reply) = connection.open_bi().await.unwrap();
    bad_send.write_all(&[0xff]).await.unwrap();
    bad_send.finish().unwrap();
    let mut ack = [0];
    tokio::time::timeout(Duration::from_secs(2), bad_reply.read_exact(&mut ack))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack, [AckCode::Rejected.as_byte()]);
    let (mut good_send, mut good_reply) = connection.open_bi().await.unwrap();
    clipboard_wire::write_frame(
        &mut good_send,
        &sample_header(),
        None,
        &Bytes::from_static(b"healthy"),
    )
    .await
    .unwrap();
    good_send.finish().unwrap();
    let delivery = tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery.message.ciphertext, Bytes::from_static(b"healthy"));
    delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    tokio::time::timeout(Duration::from_secs(2), good_reply.read_exact(&mut ack))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack, [AckCode::Accepted.as_byte()]);
    assert!(connection.close_reason().is_none());
    connection.close(0u32.into(), b"test_complete");
    harness.receiver_router.shutdown().await.unwrap();
    sender.close().await;
}

struct RecordingBackend {
    calls: AtomicUsize,
    native: IrohConnectivityBackend,
    connections: std::sync::Mutex<Vec<(usize, ConnectionMode)>>,
}
#[async_trait]
impl ConnectivityBackend for RecordingBackend {
    async fn connect(
        &self,
        target: DeviceId,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let connection = self.native.connect(target).await?;
        self.connections
            .lock()
            .unwrap()
            .push((connection.id(), connection.mode()));
        Ok(connection)
    }
}

/// 取消一个发送只终止自己的半流；同一目标的另一个发送保留活动连接。
#[tokio::test]
async fn cancelling_one_dispatch_preserves_another_active_exchange() {
    let seed = [0x51; 32];
    let sender = bind_endpoint_with(seed).await;
    let members = Arc::new(MemMemberRepo::default());
    members.save(&make_member(seed, "known")).await.unwrap();
    let mut harness = spawn_receiver([0x52; 32], members).await;
    let target = DeviceId::new("receiver");
    let addresses = Arc::new(MemPeerAddrRepo::default());
    addresses
        .upsert(&PeerAddressRecord {
            device_id: target,
            addr_blob: postcard::to_stdvec(&harness.receiver_endpoint.addr()).unwrap(),
            observed_at: Utc::now(),
        })
        .await
        .unwrap();
    let backend = Arc::new(RecordingBackend {
        calls: AtomicUsize::new(0),
        native: IrohConnectivityBackend::new(
            sender.clone(),
            addresses,
            peer_reachability_mock(),
            ConnectivityProfile::Clipboard,
        ),
        connections: std::sync::Mutex::new(Vec::new()),
    });
    let dispatch = Arc::new(super::test_support::dispatch_with_backend(
        backend.clone(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    ));
    let start = |payload: &'static [u8]| {
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            dispatch
                .dispatch(
                    &target,
                    &sample_header(),
                    SyncPayload {
                        ciphertext: Bytes::from_static(payload),
                    },
                )
                .await
        })
    };
    let first = start(b"first");
    let first_delivery = tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second = start(b"second");
    let second_delivery = tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
        .await
        .unwrap()
        .unwrap();
    first.abort();
    let _ = first.await;
    second_delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied);
    assert!(second.await.unwrap().outcome.is_ok());
    let connections = backend.connections.lock().unwrap().clone();
    assert_eq!(
        connections.len(),
        1,
        "both exchanges must use one real connection"
    );
    assert_eq!(
        connections[0].1,
        ConnectionMode::Multiplexed,
        "actual negotiated ALPN must support multiplexing"
    );
    // 发送取消不撤销已交给业务负责人的持久接受。
    assert!(first_delivery
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied));
    harness.receiver_router.shutdown().await.unwrap();
    sender.close().await;
}

struct DelayedAdmission {
    inner: EpochOnlyAdmission,
    adopting: Arc<Notify>,
    release: Arc<Semaphore>,
    begins: Arc<Semaphore>,
    adoptions: Arc<AtomicUsize>,
}
struct DelayedAttempt {
    inner: Box<dyn AdmissionAttempt>,
    adopting: Arc<Notify>,
    release: Arc<Semaphore>,
    adoptions: Arc<AtomicUsize>,
}
#[async_trait]
impl AdmissionAttempt for DelayedAttempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.inner.check_current().await
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        if self.adoptions.fetch_add(1, Ordering::SeqCst) == 0 {
            self.adopting.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        self.inner.adopt(connection).await
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for DelayedAdmission {
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        self.begins.add_permits(1);
        Box::new(DelayedAttempt {
            inner: self.inner.begin(target),
            adopting: self.adopting.clone(),
            release: self.release.clone(),
            adoptions: self.adoptions.clone(),
        })
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.inner.adopt_inbound(connection).await
    }
}

/// 准入等待期间的离线/在线不能让最新调用停在旧 flight 的失败上。
#[tokio::test]
async fn offline_during_adoption_refreshes_connection_before_any_payload_is_sent() {
    verify_liveness_recovery(true).await;
}

/// 确认离线结束旧 native 交换；新调用重新建连，连接层不自行重发旧内容。
#[tokio::test]
async fn confirmed_offline_closes_old_exchange_and_next_dispatch_dials_fresh() {
    verify_liveness_recovery(false).await;
}

async fn verify_liveness_recovery(delayed: bool) {
    let seed = [0x61; 32];
    let sender = bind_endpoint_with(seed).await;
    let members = Arc::new(MemMemberRepo::default());
    members.save(&make_member(seed, "known")).await.unwrap();
    let mut harness = spawn_receiver([0x62; 32], members).await;
    let target = DeviceId::new("receiver");
    let addresses = Arc::new(MemPeerAddrRepo::default());
    addresses
        .upsert(&PeerAddressRecord {
            device_id: target,
            addr_blob: postcard::to_stdvec(&harness.receiver_endpoint.addr()).unwrap(),
            observed_at: Utc::now(),
        })
        .await
        .unwrap();
    let (events, _) = broadcast::channel(64);
    let mut reachability = MockPeerReachability::new();
    let subscriptions = events.clone();
    reachability
        .expect_subscribe()
        .returning(move || subscriptions.subscribe());
    let backend = Arc::new(RecordingBackend {
        calls: AtomicUsize::new(0),
        native: IrohConnectivityBackend::new(
            sender.clone(),
            addresses,
            Arc::new(reachability),
            ConnectivityProfile::Clipboard,
        ),
        connections: std::sync::Mutex::new(Vec::new()),
    });
    let adopting = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(if delayed { 0 } else { 1 }));
    let begins = Arc::new(Semaphore::new(0));
    let dispatch = Arc::new(super::test_support::dispatch_with_backend(
        backend.clone(),
        Arc::new(DelayedAdmission {
            inner: EpochOnlyAdmission(AdmittedConnections::default()),
            adopting: adopting.clone(),
            release: release.clone(),
            begins: begins.clone(),
            adoptions: Arc::new(AtomicUsize::new(0)),
        }),
        ConnectivityPolicy::default(),
    ));
    let start = |payload: &'static [u8]| {
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            dispatch
                .dispatch(
                    &target,
                    &sample_header(),
                    SyncPayload {
                        ciphertext: Bytes::from_static(payload),
                    },
                )
                .await
        })
    };
    let first = start(b"first");
    tokio::time::timeout(Duration::from_secs(2), adopting.notified())
        .await
        .unwrap();
    let first_delivery = if delayed {
        None
    } else {
        let delivery = tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delivery.message.ciphertext, Bytes::from_static(b"first"));
        // 及时消费的其他 peer 事件不能积压同步游标、误关健康活动连接。
        for _ in 0..80 {
            events
                .send(PeerReachabilityChanged {
                    device_id: DeviceId::new("unrelated-peer"),
                    state: ReachabilityState::Online,
                    at: Utc::now(),
                })
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while events.len() > 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("both liveness observers must advance while the connection remains healthy");
        }
        let healthy = start(b"healthy");
        let healthy_delivery =
            tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            healthy_delivery.message.ciphertext,
            Bytes::from_static(b"healthy")
        );
        healthy_delivery
            .message
            .receipt
            .finish(InboundClipboardDisposition::Applied);
        assert_eq!(
            healthy.await.unwrap().outcome.unwrap(),
            DispatchAck::Accepted
        );
        assert!(!first.is_finished());
        assert_eq!(backend.connections.lock().unwrap().len(), 1);
        Some(delivery)
    };
    events
        .send(PeerReachabilityChanged {
            device_id: target,
            state: ReachabilityState::Offline,
            at: Utc::now(),
        })
        .unwrap();
    events
        .send(PeerReachabilityChanged {
            device_id: target,
            state: ReachabilityState::Online,
            at: Utc::now(),
        })
        .unwrap();
    // 不先等待监督任务处理离线；获取租约必须同步消费已排队的失效证据。
    let second = start(b"second");
    if delayed {
        tokio::time::timeout(Duration::from_secs(2), begins.acquire_many(2))
            .await
            .unwrap()
            .unwrap()
            .forget();
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.connections.lock().unwrap().len(), 1);
        assert!(!first.is_finished());
        assert!(!second.is_finished());
        assert!(matches!(
            harness.inbound_rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        release.add_permits(1);
        // 两个调用尚未写过负载；更新连接后各自收到自己的业务回执。
        let mut payloads = Vec::new();
        for _ in 0..2 {
            let delivery = tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
                .await
                .unwrap()
                .unwrap();
            payloads.push(delivery.message.ciphertext);
            delivery
                .message
                .receipt
                .finish(InboundClipboardDisposition::Applied);
        }
        payloads.sort();
        assert_eq!(
            payloads,
            vec![Bytes::from_static(b"first"), Bytes::from_static(b"second")]
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), first)
                .await
                .unwrap()
                .unwrap()
                .outcome
                .unwrap(),
            DispatchAck::Accepted
        );
    } else {
        let failed = tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            failed.outcome,
            Err(uc_core::ports::ClipboardDispatchError::Offline)
        ));
        let delivery = tokio::time::timeout(Duration::from_secs(2), harness.inbound_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delivery.message.ciphertext, Bytes::from_static(b"second"));
        delivery
            .message
            .receipt
            .finish(InboundClipboardDisposition::Applied);
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .unwrap()
            .unwrap()
            .outcome
            .unwrap(),
        DispatchAck::Accepted
    );
    assert!(matches!(
        harness.inbound_rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    let connections = backend.connections.lock().unwrap().clone();
    assert_eq!(
        connections.len(),
        2,
        "recovery must establish a fresh connection after confirmed offline"
    );
    assert_eq!(
        connections[0].1,
        ConnectionMode::Multiplexed,
        "actual negotiated ALPN must support multiplexing"
    );
    assert_eq!(connections[1].1, ConnectionMode::Multiplexed);
    assert_ne!(connections[0].0, connections[1].0);
    // 连接失效不撤销已交给业务负责人的持久接受。
    if let Some(first_delivery) = first_delivery {
        // 业务已经持有这份内容；连接结束可以丢弃旧 ACK 等待，不承诺回送通道仍在。
        let _ = first_delivery
            .message
            .receipt
            .finish(InboundClipboardDisposition::Applied);
    }
    harness.receiver_router.shutdown().await.unwrap();
    sender.close().await;
}

// ----- verdicts ----------------------------------------------------------

/// Verdict 1 — single frame delivered. Sender is in member_repo, frame
/// reaches the broadcast stream with ciphertext + header intact, and
/// the sender sees `Accepted`.
#[tokio::test]
async fn accepts_single_inbound_frame_from_known_peer() {
    let telemetry = test_telemetry();
    let sender_seed = [0x11u8; 32];
    let receiver_seed = [0x22u8; 32];

    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    let sender_member = make_member(sender_seed, "sender-a");
    member_repo.save(&sender_member).await.unwrap();

    let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
    let receiver_addr = harness.receiver_endpoint.addr();

    // Build the sender side.
    let sender_endpoint = bind_endpoint_with(sender_seed).await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let peer_addr_repo = Arc::new(MemPeerAddrRepo::default());
    peer_addr_repo
        .upsert(&PeerAddressRecord {
            device_id: DeviceId::new("receiver-b"),
            addr_blob: postcard::to_stdvec(&receiver_addr).expect("postcard encode EndpointAddr"),
            observed_at: Utc::now(),
        })
        .await
        .unwrap();
    let dispatch = dispatch_fixture(
        sender_endpoint,
        peer_addr_repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let payload = Bytes::from(vec![0xAB; 128]);
    let expected_payload = payload.clone();
    let client_span = operation_span(OperationContext {
        domain: DiagnosticDomain::Clipboard,
        operation: DiagnosticOperation::ClipboardDispatch,
        role: DiagnosticRole::Client,
        kind: DiagnosticSpanKind::Client,
    });
    let expected_context = client_span.context().span().span_context().clone();
    let mut dispatch_task = tokio::spawn(
        async move {
            dispatch
                .dispatch(
                    &DeviceId::new("receiver-b"),
                    &sample_header(),
                    SyncPayload {
                        ciphertext: expected_payload,
                    },
                )
                .await
                .outcome
                .expect("dispatch succeeds")
        }
        .instrument(client_span),
    );

    let mut inbound_rx = harness.inbound_rx;
    let inbound = tokio::time::timeout(Duration::from_secs(3), inbound_rx.recv())
        .await
        .expect("broadcast arrives within timeout")
        .expect("subscriber sees the frame");

    assert_eq!(inbound.message.peer_device_id.as_str(), "sender-a");
    assert_eq!(inbound.message.header, sample_header());
    assert_eq!(inbound.message.ciphertext, payload);

    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut dispatch_task)
            .await
            .is_err(),
        "sender must wait for application settlement"
    );
    assert!(inbound
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied));
    let ack = dispatch_task.await.expect("dispatch task joins");
    assert_eq!(ack, uc_core::ports::DispatchAck::Accepted);

    harness.receiver_router.shutdown().await.ok();
    telemetry.traces.force_flush().expect("trace flush");
    telemetry.logs.force_flush().expect("log flush");
    let spans = telemetry
        .span_exporter
        .get_finished_spans()
        .expect("finished spans");
    let client = spans
        .iter()
        .rev()
        .find(|span| {
            span.span_context.trace_id() == expected_context.trace_id()
                && span.span_context.span_id() == expected_context.span_id()
                && span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "uc.operation"
                        && attribute.value.as_str() == "clipboard_dispatch"
                })
        })
        .expect("client span");
    let server = spans
        .iter()
        .rev()
        .find(|span| {
            span.span_context.trace_id() == expected_context.trace_id()
                && span.parent_span_id == expected_context.span_id()
                && span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "uc.operation"
                        && attribute.value.as_str() == "clipboard_receive"
                })
        })
        .expect("server span");
    assert_eq!(
        server.span_context.trace_id(),
        client.span_context.trace_id()
    );
    assert_eq!(server.parent_span_id, client.span_context.span_id());
    let logs = telemetry
        .log_exporter
        .get_emitted_logs()
        .expect("emitted logs");
    assert!(logs.iter().any(|log| {
        log.record.trace_context().is_some_and(|context| {
            context.trace_id == server.span_context.trace_id()
                && context.span_id == server.span_context.span_id()
        })
    }));
}

#[tokio::test]
async fn old_header_layout_receives_an_explicit_incompatible_ack() {
    let sender_seed = [0x31_u8; 32];
    let receiver_seed = [0x32_u8; 32];
    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    member_repo
        .save(&make_member(sender_seed, "old-layout-sender"))
        .await
        .expect("save member");
    let harness = spawn_receiver(receiver_seed, member_repo).await;
    let sender_endpoint = bind_endpoint_with(sender_seed).await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let connection = sender_endpoint
        .connect(harness.receiver_endpoint.addr(), CLIPBOARD_ALPN)
        .await
        .expect("connect");
    let (mut send, mut recv) = connection.open_bi().await.expect("open stream");

    send.write_all(&[clipboard_wire::CLIPBOARD_MAGIC])
        .await
        .expect("magic");
    send.write_all(&1_u32.to_be_bytes()).await.expect("length");
    send.write_all(&[ClipboardHeader::CURRENT_VERSION])
        .await
        .expect("old header prefix");
    send.finish().expect("finish old frame");

    let mut ack = [0_u8; 1];
    recv.read_exact(&mut ack).await.expect("incompatible ack");
    assert_eq!(ack[0], AckCode::Incompatible.as_byte());
    let mut inbound_rx = harness.inbound_rx;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), inbound_rx.recv())
            .await
            .is_err()
    );
    drop(connection);
    harness.receiver_router.shutdown().await.ok();
}

/// Verdict 2 — unknown peer. The receiver emits `Rejected` and does
/// not push anything into the broadcast stream.
#[tokio::test]
async fn rejects_unknown_peer_with_ack_rejected() {
    let telemetry = test_telemetry();
    let sender_seed = [0x33u8; 32];
    let receiver_seed = [0x44u8; 32];

    // member_repo is empty — sender's fingerprint is unknown.
    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
    let receiver_addr = harness.receiver_endpoint.addr();

    let sender_endpoint = bind_endpoint_with(sender_seed).await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let peer_addr_repo = Arc::new(MemPeerAddrRepo::default());
    peer_addr_repo
        .upsert(&PeerAddressRecord {
            device_id: DeviceId::new("receiver-c"),
            addr_blob: postcard::to_stdvec(&receiver_addr).unwrap(),
            observed_at: Utc::now(),
        })
        .await
        .unwrap();
    let dispatch = dispatch_fixture(
        sender_endpoint,
        peer_addr_repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let client_span = operation_span(OperationContext {
        domain: DiagnosticDomain::Clipboard,
        operation: DiagnosticOperation::ClipboardDispatch,
        role: DiagnosticRole::Client,
        kind: DiagnosticSpanKind::Client,
    });
    let client_trace_id = client_span.context().span().span_context().trace_id();
    let result = dispatch
        .dispatch(
            &DeviceId::new("receiver-c"),
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"irrelevant"),
            },
        )
        .instrument(client_span)
        .await
        .outcome;

    match result {
        Err(uc_core::ports::ClipboardDispatchError::PeerRejected(msg)) => {
            assert!(
                msg.contains("Rejected ack"),
                "unexpected reject message: {msg}"
            );
        }
        other => panic!("expected PeerRejected, got {other:?}"),
    }

    // Subscriber never receives anything.
    let mut inbound_rx = harness.inbound_rx;
    let fast_poll = tokio::time::timeout(Duration::from_millis(200), inbound_rx.recv()).await;
    assert!(fast_poll.is_err(), "unknown peer must not publish");

    harness.receiver_router.shutdown().await.ok();
    telemetry.traces.force_flush().expect("trace flush");
    let spans = telemetry
        .span_exporter
        .get_finished_spans()
        .expect("finished spans");
    assert!(!spans.iter().any(|span| {
        span.span_context.trace_id() == client_trace_id
            && span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "uc.operation"
                    && attribute.value.as_str() == "clipboard_receive"
            })
    }));
}

/// Verdict 3 — a roster-resolved peer rejected by the current MLS group
/// receives a rejection ACK and cannot publish clipboard content.
#[tokio::test]
async fn rejects_known_but_unadmitted_peer_with_ack_rejected() {
    let sender_seed = [0x45u8; 32];
    let receiver_seed = [0x46u8; 32];
    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    member_repo
        .save(&make_member(sender_seed, "revoked-sender"))
        .await
        .unwrap();
    let harness =
        spawn_receiver_with_admission(receiver_seed, Arc::clone(&member_repo), false).await;
    let receiver_addr = harness.receiver_endpoint.addr();

    let sender_endpoint = bind_endpoint_with(sender_seed).await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let peer_addr_repo = Arc::new(MemPeerAddrRepo::default());
    peer_addr_repo
        .upsert(&PeerAddressRecord {
            device_id: DeviceId::new("receiver-d"),
            addr_blob: postcard::to_stdvec(&receiver_addr).unwrap(),
            observed_at: Utc::now(),
        })
        .await
        .unwrap();
    let dispatch = dispatch_fixture(
        sender_endpoint,
        peer_addr_repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let result = dispatch
        .dispatch(
            &DeviceId::new("receiver-d"),
            &sample_header(),
            SyncPayload {
                ciphertext: Bytes::from_static(b"must-not-publish"),
            },
        )
        .await
        .outcome;
    assert!(matches!(
        result,
        Err(uc_core::ports::ClipboardDispatchError::PeerRejected(_))
    ));

    let mut inbound_rx = harness.inbound_rx;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), inbound_rx.recv())
            .await
            .is_err(),
        "an unadmitted peer must not publish"
    );
    harness.receiver_router.shutdown().await.ok();
}

/// Verdict 4 — bad magic byte from a malicious / out-of-protocol peer.
/// The handler rejects the frame with `Rejected` ack and drops the
/// connection without touching the broadcast stream. Uses a raw
/// sender that writes garbage bytes to ensure the handler tolerates
/// misbehaved peers even if they otherwise pass identity resolution.
#[tokio::test]
async fn rejects_bad_magic_without_touching_broadcast_stream() {
    let sender_seed = [0x55u8; 32];
    let receiver_seed = [0x66u8; 32];

    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    let sender_member = make_member(sender_seed, "known-bad-actor");
    member_repo.save(&sender_member).await.unwrap();

    let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
    let receiver_addr = harness.receiver_endpoint.addr();

    // Raw-ish sender: bypass the dispatch adapter so we can write
    // wrong bytes on purpose.
    let sender_endpoint = bind_endpoint_with(sender_seed).await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let conn = sender_endpoint
        .connect(receiver_addr, CLIPBOARD_ALPN)
        .await
        .expect("dial receiver");
    let (mut send, mut recv) = conn.open_bi().await.expect("open_bi");
    // Bad magic + garbage; receiver should bail out at the magic check.
    send.write_all(&[0x00, 0x00, 0x00, 0x00, 0x05])
        .await
        .expect("write garbage");
    send.finish().expect("finish");

    // Receiver must respond with Rejected, not hang.
    let mut ack_buf = [0u8; 1];
    recv.read_exact(&mut ack_buf).await.expect("ack read");
    assert_eq!(ack_buf[0], AckCode::Rejected.as_byte());

    let mut inbound_rx = harness.inbound_rx;
    let fast_poll = tokio::time::timeout(Duration::from_millis(200), inbound_rx.recv()).await;
    assert!(
        fast_poll.is_err(),
        "bad magic must not publish to broadcast"
    );

    harness.receiver_router.shutdown().await.ok();
}

/// Verdict 4 — multiple concurrent inbound connections are processed
/// in parallel, and every frame shows up on the broadcast stream.
/// This is the guard against a handler that accidentally serializes
/// connections (e.g. by holding a lock across the whole `accept`
/// body).
#[tokio::test]
async fn processes_concurrent_inbound_connections() {
    let receiver_seed = [0x77u8; 32];
    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());

    // Three senders with distinct identities.
    let sender_seeds: [[u8; 32]; 3] = [[0xA0; 32], [0xA1; 32], [0xA2; 32]];
    for (i, seed) in sender_seeds.iter().enumerate() {
        let member = make_member(*seed, &format!("sender-{i}"));
        member_repo.save(&member).await.unwrap();
    }

    let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
    let receiver_addr = harness.receiver_endpoint.addr();

    let mut tasks = Vec::new();
    for (i, seed) in sender_seeds.iter().enumerate() {
        let addr = receiver_addr.clone();
        let seed = *seed;
        tasks.push(tokio::spawn(async move {
            let sender_endpoint = bind_endpoint_with(seed).await;
            wait_for_direct_addrs(&sender_endpoint).await;
            let peer_addr_repo = Arc::new(MemPeerAddrRepo::default());
            peer_addr_repo
                .upsert(&PeerAddressRecord {
                    device_id: DeviceId::new("rcv"),
                    addr_blob: postcard::to_stdvec(&addr).unwrap(),
                    observed_at: Utc::now(),
                })
                .await
                .unwrap();
            let dispatch = dispatch_fixture(
                sender_endpoint,
                peer_addr_repo,
                peer_reachability_mock(),
                Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
                ConnectivityPolicy::default(),
            );

            let mut header = sample_header();
            header.origin_device_id = format!("sender-{i}");
            let payload = Bytes::from(vec![i as u8; 32]);
            let ack = dispatch
                .dispatch(
                    &DeviceId::new("rcv"),
                    &header,
                    SyncPayload {
                        ciphertext: payload.clone(),
                    },
                )
                .await
                .outcome
                .expect("concurrent dispatch ok");
            assert_eq!(ack, uc_core::ports::DispatchAck::Accepted);
        }));
    }

    // Collect three inbound frames; order is not asserted (concurrency).
    let mut inbound_rx = harness.inbound_rx;
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..3 {
        let inbound = tokio::time::timeout(Duration::from_secs(3), inbound_rx.recv())
            .await
            .expect("broadcast arrives in time")
            .expect("subscriber sees frame");
        seen.push(inbound.message.peer_device_id.as_str().to_string());
        assert!(inbound
            .message
            .receipt
            .finish(InboundClipboardDisposition::Applied));
    }
    for task in tasks {
        task.await.expect("sender task");
    }
    seen.sort();
    assert_eq!(seen, vec!["sender-0", "sender-1", "sender-2"]);

    harness.receiver_router.shutdown().await.ok();
}

async fn bind_loopback_endpoint(seed: [u8; 32]) -> Arc<Endpoint> {
    Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(SecretKey::from_bytes(&seed))
            .alpns(vec![CLIPBOARD_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind_addr((Ipv4Addr::LOCALHOST, 0))
            .expect("loopback bind address")
            .bind()
            .await
            .expect("loopback endpoint"),
    )
}

#[derive(Debug)]
struct SelectedPathReceiver(IrohClipboardHandler);
impl ProtocolHandler for SelectedPathReceiver {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut updates = connection.paths_stream();
            loop {
                let paths = updates.next().await.expect("live native path stream");
                if paths.iter().any(|path| path.is_selected()) {
                    break;
                }
            }
        })
        .await
        .expect("native selected path must be published");
        self.0.accept(connection).await
    }
    async fn shutdown(&self) {
        self.0.shutdown().await;
    }
}

/// 一次真实投递的两端都报告该次连接 selected 的路径：回环上只有直连，
/// 发送报告与入站消息必须一致且为 Direct，且回执确认内容已被应用。
/// 路径快照允许 Unknown；仅该路径断言用例先等待真实 selected path 发布，
/// 不把地址候选/握手完成推断为已选择 Direct，也不改变生产接收策略。
/// 路径验收固定真实 loopback，避免本机虚拟网卡 fake-IP 被按生产规则分类为 Unknown。

#[tokio::test]
async fn delivery_reports_the_connection_selected_path_on_both_sides() {
    let sender_seed = [0x71u8; 32];
    let receiver_seed = [0x72u8; 32];
    let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
    member_repo
        .save(&make_member(sender_seed, "path-sender"))
        .await
        .unwrap();
    let receiver_endpoint = bind_loopback_endpoint(receiver_seed).await;
    wait_for_direct_addrs(&receiver_endpoint).await;
    let adapter = ReceiverFixture::new(
        member_table_directory(member_repo),
        Arc::new(StaticPeerAdmission(true)),
        Arc::new(Sha256IdentityFingerprintFactory),
    );
    let inbound_rx = adapter.subscribe();
    let receiver_router = Router::builder((*receiver_endpoint).clone())
        .accept(
            MULTIPLEXED_CLIPBOARD_ALPN,
            SelectedPathReceiver(adapter.handler()),
        )
        .accept(CLIPBOARD_ALPN, SelectedPathReceiver(adapter.handler()))
        .spawn();
    let harness = ReceiverHarness {
        receiver_endpoint,
        receiver_router,
        inbound_rx,
    };
    let socket = harness
        .receiver_endpoint
        .bound_sockets()
        .into_iter()
        .find(|socket| socket.ip() == Ipv4Addr::LOCALHOST)
        .expect("loopback socket");
    let receiver_addr = EndpointAddr::new(harness.receiver_endpoint.id()).with_ip_addr(socket);

    let sender_endpoint = bind_loopback_endpoint(sender_seed).await;
    wait_for_direct_addrs(&sender_endpoint).await;
    let peer_addr_repo = Arc::new(MemPeerAddrRepo::default());
    peer_addr_repo
        .upsert(&PeerAddressRecord {
            device_id: DeviceId::new("path-receiver"),
            addr_blob: postcard::to_stdvec(&receiver_addr).expect("encode EndpointAddr"),
            observed_at: Utc::now(),
        })
        .await
        .unwrap();
    let dispatch = dispatch_fixture(
        sender_endpoint,
        peer_addr_repo,
        peer_reachability_mock(),
        Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
        ConnectivityPolicy::default(),
    );

    let payload = Bytes::from(vec![0x5A; 64]);
    let expected_payload = payload.clone();
    let dispatch_task = tokio::spawn(async move {
        dispatch
            .dispatch(
                &DeviceId::new("path-receiver"),
                &sample_header(),
                SyncPayload {
                    ciphertext: expected_payload,
                },
            )
            .await
    });

    let mut inbound_rx = harness.inbound_rx;
    let inbound = tokio::time::timeout(Duration::from_secs(3), inbound_rx.recv())
        .await
        .expect("broadcast arrives within timeout")
        .expect("subscriber sees the frame");
    assert_eq!(inbound.message.ciphertext, payload);
    assert_eq!(
        inbound.message.transport,
        uc_core::ports::connection_channel::ConnectionChannel::Direct
    );
    assert!(inbound
        .message
        .receipt
        .finish(InboundClipboardDisposition::Applied));

    let report = dispatch_task.await.expect("dispatch task joins");
    assert_eq!(
        report.outcome.expect("dispatch succeeds"),
        uc_core::ports::DispatchAck::Accepted
    );
    assert_eq!(
        report.transport,
        uc_core::ports::connection_channel::ConnectionChannel::Direct
    );

    harness.receiver_router.shutdown().await.ok();
}
