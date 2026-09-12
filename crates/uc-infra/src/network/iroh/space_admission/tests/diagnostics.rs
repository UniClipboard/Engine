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
            Some(&SpaceAdmissionTransportError::Deferred),
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
async fn rejected_continuation_diagnostics_distinguish_identity_credentials_and_proof() {
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
