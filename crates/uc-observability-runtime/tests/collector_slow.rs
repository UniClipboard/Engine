use std::time::{Duration, Instant};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    managed_log_files, DeploymentEnvironment, LocalLogConfig, ObservabilityConfig,
    ObservabilityResource, OperatingSystem, OtlpHttpConfig, ProcessObservabilityRuntime,
    SignalResult,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn slow_collector_respects_the_callers_flush_deadline() {
    let receiver = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(2)))
            .mount(&receiver)
            .await;
    }
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("loopback receiver")
    .with_timeout(Duration::from_secs(1))
    .expect("bounded timeout");
    let log_directory = tempfile::tempdir().expect("local log directory");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.3",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("approved resource"),
    )
    .with_local_logs(LocalLogConfig::new(log_directory.path()))
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("install worker")
        .expect("runtime install")
        .handle();

    emit_lifecycle_completion();

    let started = Instant::now();
    let flushed = handle.force_flush(Duration::from_millis(10));
    assert_eq!(flushed.traces, SignalResult::TimedOut);
    assert_eq!(flushed.logs, SignalResult::TimedOut);
    assert!(started.elapsed() < Duration::from_millis(100));
    let shutdown = handle.shutdown(Duration::from_millis(10));
    assert_eq!(shutdown.traces, SignalResult::TimedOut);
    assert_eq!(shutdown.logs, SignalResult::TimedOut);

    let health_before_rejected_emit = handle.health();
    emit_lifecycle_completion();
    let health_after_rejected_emit = handle.health();
    assert_eq!(
        health_after_rejected_emit.dropped_remote_spans,
        health_before_rejected_emit.dropped_remote_spans
    );
    assert_eq!(
        health_after_rejected_emit.dropped_remote_logs,
        health_before_rejected_emit.dropped_remote_logs
    );

    let completed_shutdown = handle.shutdown(Duration::from_secs(5));
    assert_eq!(completed_shutdown.traces, SignalResult::Completed);
    assert_eq!(completed_shutdown.logs, SignalResult::Completed);

    let requests = receiver
        .received_requests()
        .await
        .expect("slow collector requests");
    let span_count = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .map(|request| {
            ExportTraceServiceRequest::decode(request.body.as_slice())
                .expect("trace batch")
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .map(|scope| scope.spans.len())
                .sum::<usize>()
        })
        .sum::<usize>();
    let log_count = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/logs")
        .map(|request| {
            ExportLogsServiceRequest::decode(request.body.as_slice())
                .expect("log batch")
                .resource_logs
                .into_iter()
                .flat_map(|resource| resource.scope_logs)
                .map(|scope| scope.log_records.len())
                .sum::<usize>()
        })
        .sum::<usize>();
    assert_eq!(span_count, 1);
    assert_eq!(log_count, 1);
    let final_health = handle.health();
    assert!(final_health.failed_remote_span_batches > 0);
    assert!(final_health.failed_remote_log_batches > 0);

    let local = managed_log_files(log_directory.path()).expect("managed local logs");
    let local_output = std::fs::read_to_string(&local[0]).expect("local log output");
    assert_eq!(local_output.matches("uc.operation.completed").count(), 1);
    assert!(local_output.contains("uc.observability.trace_export_failed"));
    assert!(local_output.contains("uc.observability.log_export_failed"));

    let after_completion = handle.shutdown(Duration::from_millis(10));
    assert_eq!(after_completion.traces, SignalResult::AlreadyShutdown);
    assert_eq!(after_completion.logs, SignalResult::AlreadyShutdown);
}

fn emit_lifecycle_completion() {
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Runtime,
        operation: DiagnosticOperation::SessionLifecycle,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    {
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::SessionLifecycle,
            DiagnosticRole::Local,
            Duration::ZERO,
        ));
    }
    drop(span);
}
