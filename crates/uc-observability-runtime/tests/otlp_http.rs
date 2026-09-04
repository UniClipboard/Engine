use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    managed_log_files, DeploymentEnvironment, InstallError, LocalLogConfig, ObservabilityConfig,
    ObservabilityResource, OperatingSystem, OtlpHttpConfig, ProcessObservabilityRuntime,
    SecretHeaderValue, SignalResult,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn real_otlp_http_carries_correlated_trace_and_log_with_one_resource() {
    let receiver = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&receiver)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/logs"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&receiver)
        .await;

    let remote = OtlpHttpConfig::new(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("fixture endpoints")
    .with_header(
        "authorization",
        SecretHeaderValue::new("Bearer PRIVATE_OTLP_TOKEN"),
    )
    .expect("fixture header");
    let config = ObservabilityConfig::new(ObservabilityResource::new(
        "test-version",
        DeploymentEnvironment::Test,
        OperatingSystem::Macos,
        "test-arch",
        "test-channel",
    ));
    let log_directory = tempfile::tempdir().expect("log directory");
    let config = config
        .with_local_logs(LocalLogConfig::new(log_directory.path()))
        .with_remote(remote);
    let repeated_config = config.clone();
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("install worker")
        .expect("runtime install")
        .handle();
    let reused =
        tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(repeated_config))
            .await
            .expect("reuse worker")
            .expect("same config is reused");
    assert!(format!("{reused:?}").starts_with("Reused"));
    let conflicting = ObservabilityConfig::new(ObservabilityResource::new(
        "other-version",
        DeploymentEnvironment::Test,
        OperatingSystem::Macos,
        "test-arch",
        "test-channel",
    ));
    assert!(matches!(
        ProcessObservabilityRuntime::install(conflicting),
        Err(InstallError::AlreadyInstalled)
    ));

    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
        flow: None,
    });
    {
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            Duration::from_millis(9),
        ));
        tracing::warn!(
            target: "uc_application::clipboard",
            path = "/private/sensitive-path",
            "unapproved event"
        );
    }
    drop(span);

    let flushed = handle.force_flush(Duration::from_secs(10));
    assert_eq!(flushed.traces, SignalResult::Completed);
    assert_eq!(flushed.logs, SignalResult::Completed);

    let requests = receiver
        .received_requests()
        .await
        .expect("request recording");
    let trace_request = requests
        .iter()
        .find(|request| request.url.path() == "/v1/traces")
        .expect("trace request");
    let log_request = requests
        .iter()
        .find(|request| request.url.path() == "/v1/logs")
        .expect("log request");
    assert_eq!(
        trace_request
            .headers
            .get("authorization")
            .expect("trace authorization"),
        "Bearer PRIVATE_OTLP_TOKEN"
    );
    assert_eq!(
        log_request
            .headers
            .get("authorization")
            .expect("log authorization"),
        "Bearer PRIVATE_OTLP_TOKEN"
    );

    let traces = ExportTraceServiceRequest::decode(trace_request.body.as_slice())
        .expect("decode trace payload");
    let logs =
        ExportLogsServiceRequest::decode(log_request.body.as_slice()).expect("decode log payload");
    assert_eq!(traces.resource_spans.len(), 1);
    assert_eq!(logs.resource_logs.len(), 1);
    assert_eq!(
        traces.resource_spans[0].resource,
        logs.resource_logs[0].resource
    );

    let spans = traces.resource_spans[0]
        .scope_spans
        .iter()
        .flat_map(|scope| &scope.spans)
        .collect::<Vec<_>>();
    let records = logs.resource_logs[0]
        .scope_logs
        .iter()
        .flat_map(|scope| &scope.log_records)
        .collect::<Vec<_>>();
    assert_eq!(spans.len(), 1);
    assert_eq!(records.len(), 1);
    assert!(spans[0].events.is_empty());
    assert_eq!(records[0].trace_id, spans[0].trace_id);
    assert_eq!(records[0].span_id, spans[0].span_id);

    for request in [trace_request, log_request] {
        let payload = String::from_utf8_lossy(&request.body);
        assert!(!payload.contains("PRIVATE_OTLP_TOKEN"));
        assert!(!payload.contains("/private/sensitive-path"));
        assert!(!payload.contains("private-device-name"));
    }

    let files = managed_log_files(log_directory.path()).expect("managed log files");
    assert_eq!(files.len(), 1);
    let jsonl = std::fs::read_to_string(&files[0]).expect("JSONL output");
    assert!(jsonl.contains("uc.operation.completed"));
    assert!(!jsonl.contains("/private/sensitive-path"));
    for line in jsonl.lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("each line is JSON");
    }

    let shutdown = handle.shutdown(Duration::from_secs(10));
    assert_eq!(shutdown.traces, SignalResult::Completed);
    assert_eq!(shutdown.logs, SignalResult::Completed);
    let repeated_shutdown = handle.shutdown(Duration::from_millis(1));
    assert_eq!(repeated_shutdown.traces, SignalResult::AlreadyShutdown);
    assert_eq!(repeated_shutdown.logs, SignalResult::AlreadyShutdown);
}
