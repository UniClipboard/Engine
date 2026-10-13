//! 真实 TLS/QUIC 验证：仅业务端点为 recorder，身份绑定、Uni、wire 与 local-work 使用生产实现。
use super::super::{admitted_connections::AdmittedConnections, connectivity::IrohPeerConnection};
use super::{
    HistoryConnectionAdmission, IrohMembershipHistoryExchangeHandler, MembershipHistoryWire,
    SessionMembershipHistoryLocalWork, MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
    MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
use async_trait::async_trait;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use uc_application::deps::{
    ConnectivityBackend, ConnectivityError, ConnectivityManager, ConnectivityPolicy,
    HistoryConnectionRetentionPort, IdleRetentionPolicy, KnownPeerIdentity,
    MembershipHistoryReceiver, MembershipHistoryTransport, MembershipLedgerError, PeerConnection,
    PeerIdentityDirectoryPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipHistoryAckV3, MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

struct Directory(Vec<KnownPeerIdentity>);
#[async_trait]
impl PeerIdentityDirectoryPort for Directory {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(self.0.clone())
    }
}
struct CurrentMembers(bool);
#[async_trait]
impl HistoryConnectionRetentionPort for CurrentMembers {
    async fn may_retain(&self, _: &DeviceId) -> Result<bool, MembershipLedgerError> {
        Ok(self.0)
    }
}
struct Dial {
    endpoint: Endpoint,
    target: EndpointAddr,
    facts: Mutex<Vec<(usize, Vec<u8>)>>,
}
#[async_trait]
impl ConnectivityBackend for Dial {
    async fn connect(&self, _: DeviceId) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let native = self
            .endpoint
            .connect_with_opts(
                self.target.clone(),
                MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
                iroh::endpoint::ConnectOptions::new()
                    .with_additional_alpns(vec![MEMBERSHIP_HISTORY_EXCHANGE_ALPN.to_vec()]),
            )
            .await
            .map_err(|source| ConnectivityError::Backend(Box::new(source)))?
            .await
            .map_err(|source| ConnectivityError::Backend(Box::new(source)))?;
        self.facts
            .lock()
            .unwrap()
            .push((native.stable_id(), native.alpn().to_vec()));
        Ok(IrohPeerConnection::wrap(native))
    }
}
struct Reply {
    blocked: Notify,
    release: Notify,
}
#[async_trait]
impl MembershipHistoryExchangeEndpointPort for Reply {
    async fn handle_membership_history_exchange(
        &self,
        _: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        if matches!(
            message,
            MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Diverged)
        ) {
            self.blocked.notify_one();
            self.release.notified().await;
        }
        Ok(MembershipHistoryMessage::AckV3(
            MembershipHistoryAckV3::RestrictedConsistent,
        ))
    }
}
async fn endpoint() -> Endpoint {
    let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while endpoint.addr().ip_addrs().next().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    endpoint
}
fn directory(endpoint: &Endpoint, peer: DeviceId) -> Arc<Directory> {
    Arc::new(Directory(vec![KnownPeerIdentity {
        device_id: peer,
        identity_fingerprint: Sha256IdentityFingerprintFactory
            .from_public_key(endpoint.id().as_bytes())
            .unwrap(),
    }]))
}
fn policy() -> ConnectivityPolicy {
    ConnectivityPolicy {
        idle_retention: Some(IdleRetentionPolicy {
            timeout: Duration::from_secs(30),
            capacity: 16,
        }),
        ..ConnectivityPolicy::default()
    }
}
fn request() -> MembershipHistoryMessage {
    MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid)
}

async fn run_exchange(legacy: bool, retain_server: bool, overload: bool) {
    let client = endpoint().await;
    let server = endpoint().await;
    let target = DeviceId::new("server");
    let server_admission = Arc::new(HistoryConnectionAdmission::new(
        directory(&client, DeviceId::new("client")),
        Arc::new(CurrentMembers(retain_server)),
        Arc::new(Sha256IdentityFingerprintFactory),
        AdmittedConnections::default(),
    ));
    let dial = Arc::new(Dial {
        endpoint: client.clone(),
        target: server.addr(),
        facts: Mutex::new(Vec::new()),
    });
    let client_admission = Arc::new(HistoryConnectionAdmission::new(
        directory(&server, target),
        Arc::new(CurrentMembers(true)),
        Arc::new(Sha256IdentityFingerprintFactory),
        AdmittedConnections::default(),
    ));
    let manager = ConnectivityManager::new(dial.clone(), client_admission, policy());
    let server_policy = if overload {
        ConnectivityPolicy {
            inbound_per_session: 1,
            accepted_stream_timeout: Some(Duration::from_millis(200)),
            ..policy()
        }
    } else {
        policy()
    };
    let server_manager =
        ConnectivityManager::new(dial.clone(), server_admission.clone(), server_policy);
    let reply = Arc::new(Reply {
        blocked: Notify::new(),
        release: Notify::new(),
    });
    let receiver = Arc::new(MembershipHistoryReceiver::new(
        server_manager.clone(),
        Arc::new(MembershipHistoryWire),
        server_admission,
        Arc::new(SessionMembershipHistoryLocalWork),
        reply.clone(),
    ));
    let handler = IrohMembershipHistoryExchangeHandler::new(server_manager, receiver);
    let router = if legacy {
        Router::builder(server.clone()).accept(MEMBERSHIP_HISTORY_EXCHANGE_ALPN, handler)
    } else {
        Router::builder(server.clone())
            .accept(MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN, handler.clone())
            .accept(MEMBERSHIP_HISTORY_EXCHANGE_ALPN, handler)
    };
    let router = router.spawn();
    let transport = Arc::new(MembershipHistoryTransport::new(
        manager.clone(),
        Arc::new(MembershipHistoryWire),
    ));
    assert!(transport
        .exchange_membership_history(&target, request())
        .await
        .is_ok());
    assert!(transport
        .exchange_membership_history(&target, request())
        .await
        .is_ok());
    if legacy || !retain_server {
        let facts = dial.facts.lock().unwrap();
        assert_eq!(facts.len(), 2);
        assert_ne!(facts[0].0, facts[1].0);
        let expected = if legacy {
            MEMBERSHIP_HISTORY_EXCHANGE_ALPN
        } else {
            MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN
        };
        assert!(facts.iter().all(|(_, alpn)| alpn == expected));
    } else {
        let transport_clone = transport.clone();
        let blocked = tokio::spawn(async move {
            transport_clone
                .exchange_membership_history(
                    &target,
                    MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Diverged),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), reply.blocked.notified())
            .await
            .unwrap();
        // 一个真实流尚未结算，另一流仍可独立收到完整回复。
        let following = tokio::time::timeout(
            Duration::from_secs(2),
            transport.exchange_membership_history(&target, request()),
        )
        .await
        .unwrap();
        if overload {
            assert!(
                following.is_err(),
                "overload settles only the queued stream"
            );
            reply.release.notify_one();
            assert!(
                tokio::time::timeout(Duration::from_secs(2), blocked)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_ok(),
                "in-flight brother must still complete"
            );
        } else {
            assert!(following.is_ok());
            blocked.abort();
            let _ = blocked.await;
        }
        assert!(transport
            .exchange_membership_history(&target, request())
            .await
            .is_ok());
        reply.release.notify_one();
        let facts = dial.facts.lock().unwrap();
        assert_eq!(facts.len(), 1, "actual negotiated connections: {facts:?}");
        assert_eq!(facts[0].1, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN);
    }
    manager.shutdown().await.unwrap();
    router.shutdown().await.unwrap();
    client.close().await;
    server.close().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_multistream_history_reuses_connection_and_isolates_cancelled_stream() {
    run_exchange(false, true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_legacy_history_negotiation_gives_each_exchange_a_new_connection() {
    run_exchange(true, true, false).await;
}

#[tokio::test]
async fn authenticated_remote_parent_survives_local_work_capture_without_flattening_children() {
    use opentelemetry::trace::{SpanKind, TracerProvider};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing::Instrument;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;
    use uc_observability_contract::diagnostics::{
        operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
        ObservationContext, OperationContext,
    };
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("history-context-test")));
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let remote = tracing::info_span!("remote_sender");
    let ambient = tracing::info_span!("unrelated_ambient");
    let incoming = ObservationContext::from_remote_parent(remote.context());
    incoming
        .scope(async {
            let captured = ObservationContext::capture();
            captured
                .scope(async {
                    let server = operation_span(OperationContext {
                        domain: DiagnosticDomain::SpaceMembership,
                        operation: DiagnosticOperation::MembershipHistorySync,
                        role: DiagnosticRole::Member,
                        kind: DiagnosticSpanKind::Server,
                    });
                    async {
                        let child = operation_span(OperationContext {
                            domain: DiagnosticDomain::SpaceMembership,
                            operation: DiagnosticOperation::MembershipGroupUpdate,
                            role: DiagnosticRole::Member,
                            kind: DiagnosticSpanKind::Internal,
                        });
                        async {}.instrument(child).await;
                    }
                    .instrument(server)
                    .await;
                })
                .await;
        })
        .instrument(ambient.clone())
        .await;
    drop(remote);
    drop(ambient);
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    let remote = spans
        .iter()
        .find(|span| span.name == "remote_sender")
        .unwrap();
    let server = spans
        .iter()
        .find(|span| span.span_kind == SpanKind::Server)
        .unwrap();
    assert_eq!(
        server.span_context.trace_id(),
        remote.span_context.trace_id()
    );
    assert_eq!(server.parent_span_id, remote.span_context.span_id());
    assert!(spans.iter().any(|span| span.span_kind == SpanKind::Internal
        && span.parent_span_id == server.span_context.span_id()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_hint_prevents_reusing_a_remote_restricted_scope_connection() {
    // 本端认为对方是当前成员，远端只允许临时交换；两个连续请求仍都能完成。
    run_exchange(false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overloaded_history_queue_preserves_the_in_flight_brother_and_reusable_connection() {
    run_exchange(false, true, true).await;
}
