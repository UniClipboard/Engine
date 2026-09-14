use super::super::super::space_admission_wire::{
    read_envelope, write_envelope, AuthenticatedEnvelopeV1,
};
use super::super::diagnostics::wire_failure;
use super::super::diagnostics::{handler_failure, server_completion};
use super::super::exchange::EstablishedExchange;
use super::super::server::read_peer_acknowledgement_observed;
use super::*;
use sha2::{Digest, Sha256};
use uc_application::deps::AuthenticatedAdmissionExchangePort;
use uc_core::membership::AdmissionPeerBinding;
use uc_observability_contract::diagnostics::connectivity::{
    decode_local_record, AdmissionExchangeFailure, AdmissionExchangeObservation,
    AdmissionExchangeSide, AdmissionExchangeStep,
};

#[test]
fn wire_diagnostics_classify_io_without_destroying_the_source() {
    use std::error::Error;
    let wire = WireError::Io(std::io::Error::other("sensitive diagnostic test sentinel"));
    assert_eq!(wire_failure(&wire), AdmissionExchangeFailure::IoFailed);
    let mapped = map_server_wire_error(wire);
    assert!(mapped.source().is_some());
    assert!(!mapped.to_string().contains("sensitive"));
    let timeout = WireError::Io(
        iroh::endpoint::ReadError::ConnectionLost(iroh::endpoint::ConnectionError::TimedOut).into(),
    );
    assert_eq!(wire_failure(&timeout), AdmissionExchangeFailure::TimedOut);
}

fn field<'a>(record: &'a opentelemetry_sdk::logs::SdkLogRecord, name: &str) -> Option<&'a str> {
    record
        .attributes_iter()
        .find_map(|(key, value)| match value {
            AnyValue::String(value) if key.as_str() == name => Some(value.as_str()),
            _ => None,
        })
}

pub(super) fn step_records(exporter: &InMemoryLogExporter) -> Vec<serde_json::Value> {
    exporter
        .get_emitted_logs()
        .expect("logs")
        .iter()
        .filter_map(|entry| {
            let name = field(&entry.record, "event.name")?;
            if !name.starts_with("pairing.exchange.step.") {
                return None;
            }
            let payload = field(&entry.record, "payload")?;
            let level = if payload.contains("\"failure\":null") || name.ends_with("started") {
                "INFO"
            } else {
                "WARN"
            };
            let decoded = decode_local_record(name, payload, level).expect("approved steps");
            Some(serde_json::to_value(decoded).expect("fields"))
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum ReplyScenario {
    Delayed,
    Missing,
    Closed,
    Invalid,
    WrongProof,
    StalledFinish,
    Cancelled,
}

#[tokio::test]
async fn actual_client_exchange_reports_reply_failures_and_preserves_trace_result() {
    for scenario in [
        ReplyScenario::Delayed,
        ReplyScenario::Missing,
        ReplyScenario::Closed,
        ReplyScenario::Invalid,
        ReplyScenario::WrongProof,
        ReplyScenario::StalledFinish,
        ReplyScenario::Cancelled,
    ] {
        let spans = InMemorySpanExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        let exporter = InMemoryLogExporter::default();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(traces.tracer("exchange-test"))
                    .with_context_activation(true)
                    .with_filter(filter_fn(|m| m.target() == "uc.telemetry")),
            )
            .with(
                OpenTelemetryTracingBridge::new(&logs).with_filter(filter_fn(|m| {
                    matches!(m.target(), "uc.telemetry" | "uc.connectivity")
                })),
            );
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let sponsor = bound_endpoint().await;
        wait_for_direct_addrs(&sponsor).await;
        let joiner = bound_endpoint().await;
        wait_for_direct_addrs(&joiner).await;
        let (client_connection, server_connection) = tokio::join!(
            joiner.connect(sponsor.addr(), SPACE_ADMISSION_ALPN),
            async {
                sponsor
                    .accept()
                    .await
                    .expect("incoming")
                    .await
                    .expect("accept")
            },
        );
        let client_connection = client_connection.expect("connect");
        let (send, receive) = client_connection.open_bi().await.expect("stream");
        let binding = AdmissionPeerBinding::new(
            peer_id(joiner.id().as_bytes()).expect("joiner"),
            peer_id(sponsor.id().as_bytes()).expect("sponsor"),
        )
        .expect("binding");
        let exchange = Box::new(EstablishedExchange::new(
            client_connection,
            send,
            receive,
            admission_id(),
            binding,
            credential(),
            None,
        ));
        let ready = Arc::new(Notify::new());
        let server_ready = ready.clone();
        let sponsor_peer = peer_id(sponsor.id().as_bytes()).expect("sponsor");
        let server = tokio::spawn(async move {
            let (mut send, mut receive) = server_connection.accept_bi().await.expect("stream");
            let (_, request, _) = read_envelope(&mut receive, FrameKind::Request)
                .await
                .expect("request");
            match scenario {
                ReplyScenario::Missing | ReplyScenario::Cancelled => {
                    server_ready.notify_one();
                    std::future::pending::<()>().await;
                }
                ReplyScenario::Closed => {
                    server_connection.close(0u32.into(), b"");
                    return;
                }
                ReplyScenario::Invalid => {
                    send.write_all(b"invalid!!!").await.expect("bad header");
                    send.finish().expect("finish invalid");
                    std::future::pending::<()>().await;
                }
                _ => {}
            }
            if matches!(scenario, ReplyScenario::Delayed) {
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            let canonical = request.encode_canonical_v1().expect("reply");
            let digest: [u8; 32] = Sha256::digest(&canonical).into();
            let nonce = [9; 32];
            let mut mac = calculate_mac(
                &credential(),
                b"reply",
                admission_id(),
                sponsor_peer,
                peer_id(server_connection.remote_id().as_bytes()).expect("joiner"),
                &nonce,
                &digest,
                None,
            )
            .expect("mac");
            if matches!(scenario, ReplyScenario::WrongProof) {
                mac[0] ^= 1;
            }
            write_envelope(
                &mut send,
                FrameKind::Reply,
                &AuthenticatedEnvelopeV1 {
                    nonce,
                    canonical_envelope: canonical,
                    trace_context: None,
                    mac,
                },
            )
            .await
            .expect("reply");
            if matches!(scenario, ReplyScenario::WrongProof) {
                std::future::pending::<()>().await;
            }
            read_peer_acknowledgement(&mut receive)
                .await
                .expect("acknowledgement");
            if matches!(scenario, ReplyScenario::StalledFinish) {
                server_ready.notify_one();
                std::future::pending::<()>().await;
            }
            send.finish().expect("finish reply");
            std::future::pending::<()>().await;
        });
        let request = SpaceAdmissionEnvelopeV1::new_with_version(
            SpaceAdmissionProtocolVersion::V2,
            admission_id(),
            AdmissionRole::Joiner,
            1,
            message_id(8),
            Some(message_id(7)),
            SpaceAdmissionBodyV1::CancelRequested,
        )
        .expect("request");
        let mut future = Box::pin(exchange.exchange(&request));
        let virtual_wait = matches!(
            scenario,
            ReplyScenario::Missing | ReplyScenario::StalledFinish
        );
        if virtual_wait || matches!(scenario, ReplyScenario::Cancelled) {
            tokio::select! {
                () = ready.notified() => {}
                _ = &mut future => panic!("exchange should still wait: {scenario:?}"),
            }
        }
        if virtual_wait {
            tokio::time::pause();
            tokio::time::advance(IO_DEADLINE + Duration::from_millis(1)).await;
        }
        let result = if matches!(scenario, ReplyScenario::Cancelled) {
            drop(future);
            None
        } else {
            Some(future.await)
        };
        if virtual_wait {
            tokio::time::resume();
        }
        match &result {
            Some(result) if matches!(scenario, ReplyScenario::Delayed) => assert!(result.is_ok()),
            Some(result) => assert!(result.is_err(), "{scenario:?}"),
            None => {}
        }
        server.abort();
        let _ = server.await;
        joiner.close().await;
        sponsor.close().await;
        traces.force_flush().expect("traces");
        logs.force_flush().expect("logs");
        let (step, reason, error_type) = match scenario {
            ReplyScenario::Delayed => ("wait_peer_finish", None, None),
            ReplyScenario::Missing => ("receive_reply", Some("timed_out"), Some("timeout")),
            ReplyScenario::Closed => (
                "receive_reply",
                Some("connection_closed"),
                Some("channel_closed"),
            ),
            ReplyScenario::Invalid => (
                "receive_reply",
                Some("invalid_message"),
                Some("decode_failed"),
            ),
            ReplyScenario::WrongProof => (
                "validate_reply",
                Some("authentication_rejected"),
                Some("authentication_failed"),
            ),
            ReplyScenario::StalledFinish => {
                ("wait_peer_finish", Some("timed_out"), Some("timeout"))
            }
            ReplyScenario::Cancelled => ("receive_reply", Some("interrupted"), None),
        };
        let records = step_records(&exporter);
        let last = records.last().expect("last step");
        assert_eq!(last["step"], step, "{scenario:?}");
        assert_eq!(last["error.reason"].as_str(), reason, "{scenario:?}");
        let emitted = exporter.get_emitted_logs().expect("logs");
        let completed: Vec<_> = emitted
            .iter()
            .filter(|r| field(&r.record, "event.name") == Some("uc.operation.completed"))
            .collect();
        assert_eq!(completed.len(), 1);
        assert_eq!(
            field(&completed[0].record, "error.type"),
            error_type,
            "{scenario:?}"
        );
        let spans = spans.get_finished_spans().expect("spans");
        assert_eq!(spans.len(), 1);
        let trace_error = spans[0]
            .attributes
            .iter()
            .find(|a| a.key.as_str() == "error.type")
            .map(|a| a.value.as_str());
        assert_eq!(trace_error.as_deref(), error_type);
        let context = completed[0].record.trace_context().expect("correlation");
        assert_eq!(context.span_id, spans[0].span_context.span_id());
        if matches!(scenario, ReplyScenario::Delayed) {
            let receive = records
                .iter()
                .find(|r| r["step"] == "receive_reply" && r["duration_ms"].is_number())
                .expect("receive duration");
            assert!(receive["duration_ms"].as_u64().expect("duration") >= 70);
        }
    }
}

#[tokio::test]
async fn acknowledgement_delays_and_failures_are_distinct() {
    for scenario in ["delayed", "missing", "closed", "invalid"] {
        let exporter = InMemoryLogExporter::default();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(
            OpenTelemetryTracingBridge::new(&logs).with_filter(filter_fn(|m| {
                matches!(m.target(), "uc.telemetry" | "uc.connectivity")
            })),
        );
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let (mut writer, mut reader) = tokio::io::duplex(128);
        tokio::time::pause();
        let writer = tokio::spawn(async move {
            match scenario {
                "closed" => {}
                "missing" => {
                    std::future::pending::<()>().await;
                }
                _ => {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    write_typed(
                        &mut writer,
                        FrameKind::Ack,
                        &if scenario == "invalid" { 0u8 } else { 1u8 },
                        AUTH_FRAME_LIMIT,
                    )
                    .await
                    .expect("ack");
                }
            }
        });
        let mut observation = AdmissionExchangeObservation::begin(AdmissionExchangeSide::Sponsor);
        observation.start_step(AdmissionExchangeStep::ReceiveAcknowledgement);
        let result = read_peer_acknowledgement_observed(&mut reader, &mut observation).await;
        if let Err(error) = &result {
            observation.fail(handler_failure(error));
        }
        observation.finish(server_completion(
            Duration::from_millis(80),
            result.as_ref().err(),
        ));
        writer.abort();
        let _ = writer.await;
        tokio::time::resume();
        logs.force_flush().expect("flush");
        let records = step_records(&exporter);
        assert_eq!(records.len(), 2);
        let expected = match scenario {
            "delayed" => None,
            "missing" => Some("timed_out"),
            "closed" => Some("connection_closed"),
            _ => Some("invalid_message"),
        };
        assert_eq!(records[1]["error.reason"].as_str(), expected);
        assert_eq!(result.is_ok(), scenario == "delayed");
    }
}
