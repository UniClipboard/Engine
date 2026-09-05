use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    OtlpHttpConfig, ProcessObservabilityRuntime, SignalResult,
};
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread")]
async fn tls_handshake_failure_is_reported_without_exposing_the_endpoint() {
    let plain_http_receiver = MockServer::start().await;
    let secure_base = plain_http_receiver.uri().replacen("http://", "https://", 1);
    let remote = OtlpHttpConfig::new(
        &format!("{secure_base}/v1/traces"),
        &format!("{secure_base}/v1/logs"),
    )
    .expect("HTTPS endpoints")
    .with_timeout(Duration::from_millis(500))
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
    assert!(!format!("{config:?}").contains(&secure_base));
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("install worker")
        .expect("runtime install")
        .handle();

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

    let flushed = handle.force_flush(Duration::from_secs(2));
    assert_eq!(flushed.traces, SignalResult::Failed);
    assert_eq!(flushed.logs, SignalResult::Failed);
    let health = handle.health();
    assert!(health.failed_remote_span_batches > 0);
    assert!(health.failed_remote_log_batches > 0);
    let _ = handle.shutdown(Duration::from_secs(2));
}
