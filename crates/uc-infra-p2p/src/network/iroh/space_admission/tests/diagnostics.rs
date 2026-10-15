use super::super::connection::{connect, open_stream};
use super::*;

#[test]
fn deferred_client_completion_is_not_reported_as_a_failure() {
    let log_exporter = InMemoryLogExporter::default();
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry().with(
        OpenTelemetryTracingBridge::new(&logs)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry")),
    );

    tracing::subscriber::with_default(subscriber, || {
        record_client_completion(
            DiagnosticOperation::NetworkTransport,
            Duration::from_millis(7),
            Some(&SpaceAdmissionTransportError::deferred()),
        );
    });
    logs.force_flush().expect("log flush");

    let emitted = log_exporter.get_emitted_logs().expect("emitted logs");
    assert_eq!(emitted.len(), 1);
    let record = &emitted[0].record;
    assert!(record.attributes_iter().any(|(key, value)| {
        key.as_str() == "uc.outcome"
            && matches!(value, AnyValue::String(value) if value.as_str() == "deferred")
    }));
    assert!(!record
        .attributes_iter()
        .any(|(key, _)| key.as_str() == "error.type"));
}

#[test]
fn pre_authentication_failure_is_one_unassociated_log_without_a_span() {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let traces = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let trace_layer = tracing_opentelemetry::layer()
        .with_tracer(traces.tracer("space-admission-pre-auth-test"))
        .with_context_activation(true)
        .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
    let log_layer = OpenTelemetryTracingBridge::new(&logs)
        .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
    let subscriber = tracing_subscriber::registry()
        .with(trace_layer)
        .with(log_layer);

    tracing::subscriber::with_default(subscriber, || {
        complete_admission_authentication_failure(
            AuthenticationFailure::InitialProof(ProofFailure::Rejected),
            Duration::from_secs(30),
        );
    });
    traces.force_flush().expect("trace flush");
    logs.force_flush().expect("log flush");

    assert!(span_exporter
        .get_finished_spans()
        .expect("finished spans")
        .is_empty());
    let emitted = log_exporter.get_emitted_logs().expect("emitted logs");
    assert_eq!(emitted.len(), 1);
    let record = &emitted[0].record;
    assert!(record.trace_context().is_none());
    assert!(record.body().is_none());
    assert!(record.attributes_iter().any(|(key, value)| {
        key.as_str() == "error.type"
            && matches!(value, AnyValue::String(value) if value.as_str() == "authentication_failed")
    }));
    assert!(record.attributes_iter().any(|(key, value)| {
        key.as_str() == "duration_ms" && matches!(value, AnyValue::Int(30_000))
    }));
}

#[tokio::test]
async fn stalled_pre_authentication_records_one_unassociated_timeout() {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let traces = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let trace_layer = tracing_opentelemetry::layer()
        .with_tracer(traces.tracer("space-admission-stalled-auth-test"))
        .with_context_activation(true)
        .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
    let log_layer = OpenTelemetryTracingBridge::new(&logs).with_filter(filter_fn(|metadata| {
        matches!(metadata.target(), "uc.telemetry" | "uc.connectivity")
    }));
    let subscriber = tracing_subscriber::registry()
        .with(trace_layer)
        .with(log_layer);
    let _subscriber = tracing::subscriber::set_default(subscriber);

    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let endpoint = Arc::new(HangingLoopbackEndpoint {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let credentials = Arc::new(LoopbackCredentials {
        initial: Mutex::new(None),
        continuation: Mutex::new(None),
    });
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
            .expect("handler")
            .with_exchange_deadline(Duration::from_millis(100)),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler)
        .spawn();
    let connection = connect(&joiner, sponsor.addr())
        .await
        .expect("connect stalled peer");
    let (_send, _receive) = open_stream(&connection).await.expect("open stalled stream");
    tokio::time::timeout(Duration::from_secs(2), connection.closed())
        .await
        .expect("server deadline must close the stalled peer");
    assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);

    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;
    traces.force_flush().expect("trace flush");
    logs.force_flush().expect("log flush");

    assert!(span_exporter
        .get_finished_spans()
        .expect("finished spans")
        .is_empty());
    let emitted = log_exporter.get_emitted_logs().expect("emitted logs");
    let completed: Vec<_> = emitted.iter().filter(|entry| entry.record.attributes_iter().any(|(key, value)| {
        key.as_str() == "event.name" && matches!(value, AnyValue::String(value) if value.as_str() == "uc.operation.completed")
    })).collect();
    assert_eq!(completed.len(), 1);
    let record = &completed[0].record;
    assert!(record.trace_context().is_none());
    assert!(record.attributes_iter().any(|(key, value)| {
        key.as_str() == "error.type"
            && matches!(value, AnyValue::String(value) if value.as_str() == "timeout")
    }));
    assert!(record.attributes_iter().any(|(key, value)| {
        key.as_str() == "duration_ms"
            && matches!(value, AnyValue::Int(duration) if *duration >= 100)
    }));
}

#[tokio::test]
async fn a_sponsor_at_capacity_records_one_busy_warning_without_peer_identity() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let endpoint = Arc::new(HangingLoopbackEndpoint {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let credentials = Arc::new(LoopbackCredentials {
        initial: Mutex::new(None),
        continuation: Mutex::new(None),
    });
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
            .expect("handler")
            .with_capacity(0),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler)
        .spawn();
    let connection = connect(&joiner, sponsor.addr())
        .await
        .expect("connect to a full sponsor");
    tokio::time::timeout(Duration::from_secs(2), connection.closed())
        .await
        .expect("a full sponsor closes the connection");
    assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);

    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;

    assert_eq!(
        logs.count("rejected while sponsor is at capacity"),
        1,
        "{}",
        logs.output()
    );
    // 传输层自己的 TRACE 会带端点标识，这里只检查本条业务记录。
    let output = logs.output();
    let record = output
        .lines()
        .find(|line| line.contains("rejected while sponsor is at capacity"))
        .expect("busy record");
    assert!(record.contains("reason=\"busy\""), "{record}");
    assert!(!record.contains(&joiner.id().to_string()), "{record}");
}

#[derive(Debug)]
struct DetailProbe(Arc<std::sync::Mutex<Vec<(&'static str, &'static str)>>>);
impl opentelemetry_sdk::logs::LogProcessor for DetailProbe {
    fn emit(
        &self,
        data: &mut opentelemetry_sdk::logs::SdkLogRecord,
        _: &opentelemetry::InstrumentationScope,
    ) {
        let field = |name: &str| {
            data.attributes_iter()
                .find_map(|(key, value)| {
                    if key.as_str() == name {
                        if let AnyValue::String(value) = value {
                            return Some(value.as_str());
                        }
                    }
                    None
                })
                .unwrap_or_default()
        };
        if let Some(detail) =
            uc_observability_contract::diagnostics::connectivity::take_local_completion_detail(
                field("uc.domain"),
                field("uc.operation"),
                field("uc.role"),
                field("uc.outcome"),
            )
        {
            self.0.lock().expect("details").push(detail.local_fields());
        }
    }
    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }
}

#[tokio::test]
async fn rejected_continuation_diagnostics_distinguish_identity_credentials_and_proof() {
    for (wrong_identity, stored_credential, expected_stage, expected_reason) in [
        (true, None, "continuation_identity", "identity_mismatch"),
        (
            false,
            None,
            "continuation_credential",
            "storage_unavailable",
        ),
        (
            false,
            Some(vec![7; 64]),
            "continuation_proof",
            "proof_rejected",
        ),
    ] {
        let exporter = InMemoryLogExporter::default();
        let details = Arc::new(std::sync::Mutex::new(Vec::new()));
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .with_log_processor(DetailProbe(details.clone()))
            .build();
        let subscriber = tracing_subscriber::registry().with(
            OpenTelemetryTracingBridge::new(&logs)
                .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry")),
        );
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let sponsor = bound_endpoint().await;
        wait_for_direct_addrs(&sponsor).await;
        let joiner = bound_endpoint().await;
        wait_for_direct_addrs(&joiner).await;
        let endpoint = Arc::new(HangingLoopbackEndpoint {
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
        });
        let credentials = Arc::new(LoopbackCredentials {
            initial: Mutex::new(None),
            continuation: Mutex::new(stored_credential),
        });
        let handler = Arc::new(
            IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
                .expect("handler"),
        );
        let router = Router::builder((*sponsor).clone())
            .accept(SPACE_ADMISSION_ALPN, handler)
            .spawn();
        let connection = connect(&joiner, sponsor.addr()).await.expect("connection");
        let (mut send, _receive) = open_stream(&connection).await.expect("stream");
        write_typed(
            &mut send,
            FrameKind::ContinuationHello,
            &ContinuationHelloV1 {
                admission_id: [3; 32],
                local_peer_id: if wrong_identity {
                    [9; 32]
                } else {
                    *joiner.id().as_bytes()
                },
                remote_peer_id: *sponsor.id().as_bytes(),
                nonce: [4; 32],
                request_digest: [0; 32],
                mac: vec![0; 64],
            },
            AUTH_FRAME_LIMIT,
        )
        .await
        .expect("hello");
        let closed = tokio::time::timeout(Duration::from_secs(2), connection.closed())
            .await
            .expect("rejected");
        assert!(
            matches!(closed, iroh::endpoint::ConnectionError::ApplicationClosed(ref close) if close.error_code == CLOSE_AUTHENTICATION.into())
        );
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);
        router.shutdown().await.expect("router");
        joiner.close().await;
        sponsor.close().await;
        logs.force_flush().expect("flush");
        let records = exporter.get_emitted_logs().expect("diagnostics");
        assert_eq!(records.len(), 1);
        assert_eq!(
            *details.lock().expect("details"),
            vec![(expected_stage, expected_reason)]
        );
        let record = &records[0].record;
        assert!(record.attributes_iter().any(|(key, value)| key.as_str() == "error.type" && matches!(value, AnyValue::String(value) if value.as_str() == "authentication_failed")));
        assert!(record.body().is_none());
        assert!(record.trace_context().is_none());
    }
}

#[tokio::test]
async fn old_joiner_version_is_reported_as_hello_upgrade_not_identity_mismatch() {
    let exporter = InMemoryLogExporter::default();
    let details = Arc::new(std::sync::Mutex::new(Vec::new()));
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .with_log_processor(DetailProbe(details.clone()))
        .build();
    let subscriber = tracing_subscriber::registry().with(
        OpenTelemetryTracingBridge::new(&logs)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry")),
    );
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let endpoint = Arc::new(HangingLoopbackEndpoint {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let credentials = Arc::new(LoopbackCredentials {
        initial: Mutex::new(None),
        continuation: Mutex::new(None),
    });
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials).expect("handler"),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler)
        .spawn();
    let connection = connect(&joiner, sponsor.addr()).await.expect("connection");
    let (mut send, _receive) = open_stream(&connection).await.expect("stream");
    write_typed(
        &mut send,
        FrameKind::InitialHello,
        &InitialHelloV2 {
            protocol_version: SpaceAdmissionProtocolVersion::V2.as_u16(),
            admission_id: [0x71; 32],
            invitation_id: [0x72; 32],
            joiner_peer_id: *joiner.id().as_bytes(),
            attempt_started_at_ms: 1_000,
            attempt_expires_at_ms: 301_000,
            ke1: vec![0x73],
        },
        AUTH_FRAME_LIMIT,
    )
    .await
    .expect("old hello");
    let closed = tokio::time::timeout(Duration::from_secs(2), connection.closed())
        .await
        .expect("rejected");
    assert!(
        matches!(closed, iroh::endpoint::ConnectionError::ApplicationClosed(ref close) if close.error_code == CLOSE_PEER_UPGRADE_REQUIRED.into())
    );
    router.shutdown().await.expect("router");
    joiner.close().await;
    sponsor.close().await;
    logs.force_flush().expect("flush");
    let records = exporter.get_emitted_logs().expect("diagnostics");
    assert_eq!(records.len(), 1);
    assert_eq!(
        *details.lock().expect("details"),
        vec![("receive_hello", "peer_upgrade_required")]
    );
    assert!(records[0]
        .record
        .attributes_iter()
        .any(|(key, value)| key.as_str() == "error.type"
            && matches!(value, AnyValue::String(value) if value.as_str() == "peer_incompatible")));
}

#[test]
fn authenticated_context_builds_a_real_client_server_parent() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("space-admission-trace-test"))
            .with_context_activation(true),
    );
    let admission = admission_id();

    tracing::subscriber::with_default(subscriber, || {
        let client = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let _client_entered = client.enter();
        let wire_context = inject_current().expect("client W3C context");
        let credential = credential();
        let sender = peer(0x33);
        let receiver = peer(0x34);
        let nonce = [0x35; 32];
        let digest = [0x36; 32];
        let mac = calculate_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &nonce,
            &digest,
            Some(&wire_context),
        )
        .expect("context-bound MAC");
        verify_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &nonce,
            &digest,
            Some(&wire_context),
            &mac,
        )
        .expect("authenticated context");
        let server = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Sponsor,
            kind: DiagnosticSpanKind::Server,
        });
        assert!(set_remote_parent(&server, Some(&wire_context)));
        let _server_entered = server.enter();
    });
    provider.force_flush().expect("trace flush");

    let spans = exporter.get_finished_spans().expect("finished spans");
    let client = spans
        .iter()
        .find(|span| {
            span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "uc.role" && attribute.value.as_str() == "joiner"
            })
        })
        .expect("client span");
    let server = spans
        .iter()
        .find(|span| {
            span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "uc.role" && attribute.value.as_str() == "sponsor"
            })
        })
        .expect("server span");
    assert!(!server
        .attributes
        .iter()
        .any(|attribute| attribute.key.as_str() == "uc.flow.id"));
    assert_eq!(
        server.span_context.trace_id(),
        client.span_context.trace_id()
    );
    assert_eq!(server.parent_span_id, client.span_context.span_id());
}

/// 对真实 joiner 客户端回放一个在 `OpaqueResponse` 之前就出问题的 sponsor，返回本地详情与完成记录的错误类别。
async fn joiner_response_failure(
    handler: impl ProtocolHandler,
) -> (
    SpaceAdmissionTransportError,
    Vec<(&'static str, &'static str)>,
    Vec<String>,
) {
    let exporter = InMemoryLogExporter::default();
    let details = Arc::new(std::sync::Mutex::new(Vec::new()));
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .with_log_processor(DetailProbe(details.clone()))
        .build();
    let subscriber = tracing_subscriber::registry().with(
        OpenTelemetryTracingBridge::new(&logs)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry")),
    );
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler)
        .spawn();
    let invitation = InvitationId::from_bytes([0x71; 32]).expect("invitation id");
    let derived = SpaceAdmissionAuth::derive_password_equivalent(b"response-failure", invitation);
    let password = AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
        .expect("password equivalent");
    let route = SpaceAdmissionRoute::from_bytes(
        encode_space_admission_route(&sponsor.addr(), Some(invitation)).expect("route encoding"),
    )
    .expect("route");
    let transport = IrohSpaceAdmissionTransport::new(joiner.clone());
    let attempt = transport.establish_initial(
        SpaceAdmissionId::from_bytes([0x72; 32]).expect("admission id"),
        AdmissionAttemptTimeline::start(1_000).expect("valid attempt timeline"),
        &route,
        &password,
    );
    let error = match attempt.await {
        Ok(_) => panic!("the sponsor never completes the response"),
        Err(error) => error,
    };
    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;
    logs.force_flush().expect("flush");
    let error_types = exporter
        .get_emitted_logs()
        .expect("records")
        .iter()
        .filter_map(|entry| {
            entry.record.attributes_iter().find_map(|(key, value)| {
                (key.as_str() == "error.type").then(|| match value {
                    AnyValue::String(value) => value.as_str().to_owned(),
                    _ => String::new(),
                })
            })
        })
        .collect();
    let details = details.lock().expect("details").clone();
    (error, details, error_types)
}

/// 读到 hello 后的 sponsor 行为：不回应、重置发送流、发出损坏帧、以关闭码关闭。
#[derive(Debug, Clone, Copy)]
enum ResponseBehavior {
    Silent,
    ResetStream,
    GarbageFrame,
    Close(u32),
}

#[derive(Debug)]
struct MisbehavingSponsor(ResponseBehavior);

impl ProtocolHandler for MisbehavingSponsor {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) = connection.accept_bi().await.expect("sponsor stream");
        read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
            .await
            .expect("initial hello");
        match self.0 {
            ResponseBehavior::Silent => std::future::pending().await,
            ResponseBehavior::ResetStream => {
                send.reset(0_u8.into()).expect("reset");
                connection.closed().await;
            }
            ResponseBehavior::GarbageFrame => {
                send.write_all(&[0xff; 16]).await.expect("garbage");
                connection.closed().await;
            }
            ResponseBehavior::Close(code) => {
                connection.close(code.into(), b"test");
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn joiner_distinguishes_why_the_opaque_response_never_arrived() {
    for (behavior, expected, error_type) in [
        (
            ResponseBehavior::ResetStream,
            "stream_reset",
            Some("channel_closed"),
        ),
        (
            ResponseBehavior::GarbageFrame,
            "decode",
            Some("decode_failed"),
        ),
        (
            ResponseBehavior::Close(CLOSE_AUTHENTICATION),
            "app_close_authentication",
            Some("authentication_failed"),
        ),
        // busy 的公开结果是延期，完成记录没有错误类别。
        (ResponseBehavior::Close(CLOSE_BUSY), "app_close_busy", None),
    ] {
        let (error, details, error_types) =
            joiner_response_failure(MisbehavingSponsor(behavior)).await;
        assert_eq!(
            details,
            vec![("receive_response", expected)],
            "{behavior:?}: {error:?}"
        );
        assert_eq!(
            error_types,
            error_type
                .map(str::to_owned)
                .into_iter()
                .collect::<Vec<_>>(),
            "{behavior:?}"
        );
        // 只有读响应超时有独立结果；其余失败（含真实的认证关闭）仍是认证被拒绝，busy 是延期。
        let busy = matches!(behavior, ResponseBehavior::Close(code) if code == CLOSE_BUSY);
        assert!(
            if busy {
                matches!(error, SpaceAdmissionTransportError::Deferred { .. })
            } else {
                matches!(
                    error,
                    SpaceAdmissionTransportError::AuthenticationRejected { .. }
                )
            },
            "{behavior:?}: {error:?}"
        );
    }
}

/// 真实等待读响应的 30 秒截止：沉默的 sponsor 必须被记录为响应超时，而不是认证失败。
#[tokio::test]
async fn silent_sponsor_is_recorded_as_response_timeout_not_authentication_failure() {
    let (error, details, error_types) =
        joiner_response_failure(MisbehavingSponsor(ResponseBehavior::Silent)).await;
    // 沉默的 sponsor 有自己的公开结果，不再是认证被拒绝（口令错误）。
    assert!(
        matches!(error, SpaceAdmissionTransportError::SponsorUnresponsive),
        "{error:?}"
    );
    assert_eq!(details, vec![("receive_response", "response_timeout")]);
    assert_eq!(error_types, vec!["timeout".to_owned()]);
}

/// 会话门关闭（`session_unavailable` / `session_retired`，关闭码 0）说明 sponsor 当前没有可用会话，
/// 不是认证失败：joiner 应当延期重试，而不是被当作口令错误。
#[tokio::test]
async fn session_gate_close_before_the_response_defers_the_joiner() {
    let (error, details, error_types) =
        joiner_response_failure(MisbehavingSponsor(ResponseBehavior::Close(0))).await;

    assert!(
        matches!(error, SpaceAdmissionTransportError::Deferred { .. }),
        "{error:?}"
    );
    assert_eq!(
        details,
        vec![("receive_response", "app_close_session_unavailable")]
    );
    assert!(error_types.is_empty(), "a deferral is not an error");
}
