use super::*;

#[test]
fn old_layout_and_close_codes_keep_upgrade_authentication_and_protocol_distinct() {
    assert!(matches!(
        map_request_wire_error(WireError::UnsupportedLayout),
        HandlerError::PeerUpgradeRequired
    ));
    assert!(matches!(
        map_reply_wire_error(WireError::UnsupportedLayout),
        SpaceAdmissionTransportError::PeerUpgradeRequired
    ));
    assert!(matches!(
        map_application_close_code(u64::from(CLOSE_PEER_UPGRADE_REQUIRED)),
        Some(SpaceAdmissionTransportError::PeerUpgradeRequired)
    ));
    assert!(matches!(
        map_application_close_code(u64::from(CLOSE_AUTHENTICATION)),
        Some(SpaceAdmissionTransportError::AuthenticationRejected)
    ));
    assert!(matches!(
        map_application_close_code(u64::from(CLOSE_BUSY)),
        Some(SpaceAdmissionTransportError::Deferred)
    ));
    assert!(map_application_close_code(u64::from(CLOSE_PROTOCOL)).is_none());
    assert!(matches!(
        map_application_close_code(u64::from(LEGACY_CLOSE_PROTOCOL)),
        Some(SpaceAdmissionTransportError::PeerUpgradeRequired)
    ));
    assert!(matches!(
        map_server_wire_error(WireError::Timeout),
        HandlerError::Timeout
    ));
}

#[tokio::test]
async fn server_rejects_missing_and_invalid_peer_acknowledgements() {
    let (writer, mut reader) = tokio::io::duplex(128);
    drop(writer);
    assert!(matches!(
        read_peer_acknowledgement(&mut reader).await,
        Err(HandlerError::Acknowledgement)
    ));

    let (mut writer, mut reader) = tokio::io::duplex(128);
    write_typed(&mut writer, FrameKind::Ack, &0_u8, AUTH_FRAME_LIMIT)
        .await
        .expect("invalid acknowledgement frame");
    assert!(matches!(
        read_peer_acknowledgement(&mut reader).await,
        Err(HandlerError::Protocol)
    ));
    assert_eq!(
        server_error_type(&HandlerError::Acknowledgement),
        DiagnosticErrorType::ChannelClosed
    );
    assert_eq!(
        server_error_type(&HandlerError::Timeout),
        DiagnosticErrorType::Timeout
    );
}

#[tokio::test]
async fn server_classifies_a_stalled_peer_acknowledgement_as_timeout() {
    let (_writer, mut reader) = tokio::io::duplex(128);
    tokio::time::pause();
    let acknowledgement = read_peer_acknowledgement(&mut reader);
    tokio::pin!(acknowledgement);
    tokio::select! {
        biased;
        _ = &mut acknowledgement => panic!("stalled acknowledgement completed early"),
        () = tokio::task::yield_now() => {}
    }
    tokio::time::advance(IO_DEADLINE + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;

    assert!(matches!(acknowledgement.await, Err(HandlerError::Timeout)));
}

#[tokio::test]
async fn new_client_maps_a_real_legacy_layout_server_to_peer_upgrade_required() {
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let invitation = InvitationId::from_bytes([0x61; 32]).expect("invitation id");
    let admission = SpaceAdmissionId::from_bytes([0x62; 32]).expect("admission id");
    let derived = SpaceAdmissionAuth::derive_password_equivalent(b"legacy-pass", invitation);
    let setup = SpaceAdmissionAuth::generate_server_setup();
    let registration = SpaceAdmissionAuth::register_password_equivalent(&setup, &derived)
        .expect("legacy registration");
    let credentials = Arc::new(LoopbackCredentials {
        initial: Mutex::new(Some(SponsorOpaqueMaterial::new(setup, registration))),
        continuation: Mutex::new(None),
    });
    let legacy = LegacyLayoutHandler {
        local_peer_id: peer_id(sponsor.id().as_bytes()).expect("legacy sponsor peer"),
        credentials,
    };
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, legacy)
        .spawn();
    let password = AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
        .expect("password equivalent");
    let route = SpaceAdmissionRoute::from_bytes(
        encode_space_admission_route(&sponsor.addr(), Some(invitation)).expect("route encoding"),
    )
    .expect("route");

    let mut exchange = IrohSpaceAdmissionTransport::new(joiner.clone())
        .establish_initial(admission, &route, &password)
        .await
        .expect("legacy peer authenticates before layout detection");
    let _ = exchange.take_newly_established_continuation();
    let result = exchange
        .exchange(&join_request(admission, invitation))
        .await;

    assert!(matches!(
        result,
        Err(SpaceAdmissionTransportError::PeerUpgradeRequired)
    ));
    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;
}

#[tokio::test]
async fn stalled_authenticated_endpoint_records_one_server_timeout() {
    let exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let log_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let trace_layer = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer("space-admission-timeout-test"))
        .with_context_activation(true)
        .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
    let log_layer = OpenTelemetryTracingBridge::new(&log_provider)
        .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
    let subscriber = tracing_subscriber::registry()
        .with(trace_layer)
        .with(log_layer);
    let _subscriber = tracing::subscriber::set_default(subscriber);

    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let invitation = InvitationId::from_bytes([0x71; 32]).expect("invitation id");
    let admission = SpaceAdmissionId::from_bytes([0x72; 32]).expect("admission id");
    let derived = SpaceAdmissionAuth::derive_password_equivalent(b"timeout-pass", invitation);
    let setup = SpaceAdmissionAuth::generate_server_setup();
    let registration =
        SpaceAdmissionAuth::register_password_equivalent(&setup, &derived).expect("registration");
    let credentials = Arc::new(LoopbackCredentials {
        initial: Mutex::new(Some(SponsorOpaqueMaterial::new(setup, registration))),
        continuation: Mutex::new(None),
    });
    let endpoint = Arc::new(HangingLoopbackEndpoint {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
            .expect("handler")
            .with_exchange_deadline(Duration::from_secs(30)),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler)
        .spawn();
    let password = AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
        .expect("password equivalent");
    let route = SpaceAdmissionRoute::from_bytes(
        encode_space_admission_route(&sponsor.addr(), Some(invitation)).expect("route encoding"),
    )
    .expect("route");
    let exchange = IrohSpaceAdmissionTransport::new(joiner.clone())
        .establish_initial(admission, &route, &password)
        .await
        .expect("initial authentication");

    let entered = endpoint.entered.notified();
    tokio::pin!(entered);
    let request = join_request(admission, invitation);
    let exchange = exchange.exchange(&request);
    tokio::pin!(exchange);
    tokio::select! {
        () = &mut entered => {}
        _ = &mut exchange => panic!("endpoint must stall before completion"),
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    let result = tokio::time::timeout(Duration::from_secs(1), &mut exchange)
        .await
        .expect("server deadline must close the exchange");
    assert!(result.is_err());
    assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);

    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;
    provider.force_flush().expect("trace flush");
    log_provider.force_flush().expect("log flush");
    let server_spans = exporter
        .get_finished_spans()
        .expect("finished spans")
        .into_iter()
        .filter(|span| {
            span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "uc.role" && attribute.value.as_str() == "sponsor"
            }) && span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "uc.operation"
                    && attribute.value.as_str() == "network_transport"
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(server_spans.len(), 1);
    assert_eq!(
        server_spans[0].span_kind,
        opentelemetry::trace::SpanKind::Server
    );
    assert!(server_spans[0].attributes.iter().any(|attribute| {
        attribute.key.as_str() == "uc.domain" && attribute.value.as_str() == "space_admission"
    }));
    assert!(matches!(
        server_spans[0].status,
        opentelemetry::trace::Status::Error { .. }
    ));
    let timeout_logs = log_exporter
        .get_emitted_logs()
        .expect("emitted logs")
        .into_iter()
        .filter(|log| {
            let matches_span = log.record.trace_context().is_some_and(|context| {
                context.trace_id == server_spans[0].span_context.trace_id()
                    && context.span_id == server_spans[0].span_context.span_id()
            });
            let is_timeout = log.record.attributes_iter().any(|(key, value)| {
                key.as_str() == "error.type"
                    && matches!(value, AnyValue::String(value) if value.as_str() == "timeout")
            });
            matches_span && is_timeout
        })
        .collect::<Vec<_>>();
    assert_eq!(timeout_logs.len(), 1);
    assert!(timeout_logs[0].record.body().is_none());
    assert!(!timeout_logs[0]
        .record
        .attributes_iter()
        .any(|(key, _)| key.as_str() == "uc.flow.id"));
}

#[tokio::test]
async fn real_iroh_loopback_runs_initial_and_continuation_typed_exchanges() {
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    wait_for_direct_addrs(&joiner).await;
    let invitation = InvitationId::from_bytes([0x51; 32]).expect("invitation id");
    let admission = SpaceAdmissionId::from_bytes([0x52; 32]).expect("admission id");
    let derived = SpaceAdmissionAuth::derive_password_equivalent(b"loopback-pass", invitation);
    let setup = SpaceAdmissionAuth::generate_server_setup();
    let registration =
        SpaceAdmissionAuth::register_password_equivalent(&setup, &derived).expect("registration");
    let credentials = Arc::new(LoopbackCredentials {
        initial: Mutex::new(Some(SponsorOpaqueMaterial::new(setup, registration))),
        continuation: Mutex::new(None),
    });
    let route_bytes =
        encode_space_admission_route(&sponsor.addr(), Some(invitation)).expect("route encoding");
    let route = SpaceAdmissionRoute::from_bytes(route_bytes.clone()).expect("route");
    let endpoint = Arc::new(PersistingLoopbackEndpoint {
        credentials: Arc::clone(&credentials),
        candidate_state: Mutex::new(None),
        calls: AtomicUsize::new(0),
        completed: AtomicUsize::new(0),
        continuation_route: route_bytes,
    });
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials).expect("handler"),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, Arc::clone(&handler))
        .spawn();
    let transport = IrohSpaceAdmissionTransport::new(joiner.clone());
    let password = AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
        .expect("password equivalent");
    let mut initial = transport
        .establish_initial(admission, &route, &password)
        .await
        .expect("initial OPAQUE");
    let binding = initial.peer_binding();
    let continuation = initial
        .take_newly_established_continuation()
        .expect("new continuation");
    let join_request = join_request(admission, invitation);
    let candidate_result = initial.exchange(&join_request).await;
    assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);
    assert_eq!(endpoint.completed.load(Ordering::SeqCst), 1);
    let saved = endpoint
        .candidate_state
        .lock()
        .await
        .clone()
        .expect("candidate state persisted");
    let saved = SponsorAdmission::decode_persisted(&saved).expect("candidate state decodes");
    let saved_reply = saved.current_exact_reply().expect("candidate reply saved");
    let canonical = saved_reply
        .encode_canonical_v1()
        .expect("candidate encodes");
    SpaceAdmissionEnvelopeV1::decode_canonical_v1(&canonical)
        .expect("candidate canonical reply decodes");
    let candidate = candidate_result.expect("Candidate reply");
    let (candidate, _) = candidate.into_parts();
    assert_eq!(
        candidate.kind(),
        uc_core::membership::SpaceAdmissionMessageKind::Candidate
    );

    let prepared = prepared_request(admission, &candidate);
    let resumed = transport
        .resume(admission, &route, binding, &continuation)
        .await
        .expect("continuation authentication");
    let commit = resumed.exchange(&prepared).await.expect("Commit reply");
    let (commit, _) = commit.into_parts();
    assert_eq!(
        commit.kind(),
        uc_core::membership::SpaceAdmissionMessageKind::Commit
    );
    assert_eq!(endpoint.calls.load(Ordering::SeqCst), 2);

    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;
}
