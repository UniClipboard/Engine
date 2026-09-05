use std::time::{Duration, Instant};

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    OtlpHttpConfig, ProcessObservabilityRuntime, SignalResult,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn collector_rejection_is_reported_without_blocking_the_operation() {
    let receiver = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(401))
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
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.3",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("approved resource"),
    )
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("install worker")
        .expect("runtime install")
        .handle();

    let started = Instant::now();
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
    assert!(started.elapsed() < Duration::from_millis(100));

    let flushed = handle.force_flush(Duration::from_secs(2));
    assert!(matches!(
        flushed.traces,
        SignalResult::Completed | SignalResult::Failed
    ));
    assert!(matches!(
        flushed.logs,
        SignalResult::Completed | SignalResult::Failed
    ));
    let health = handle.health();
    assert!(health.failed_remote_span_batches > 0);
    assert!(health.failed_remote_log_batches > 0);
    let _ = handle.shutdown(Duration::from_secs(2));
}
