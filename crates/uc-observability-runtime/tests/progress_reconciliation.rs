//! 新核对动作的固定名称、结果与隐私必须经过真实编码和HTTP导出。
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{any_value::Value, KeyValue};
use prost::Message;
use std::time::Duration;
use uc_observability_contract::diagnostics::{
    independent_operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationContext, ProgressReconciliationOutcome,
};
use uc_observability_runtime::{
    DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    OtlpHttpConfig, ProcessObservabilityRuntime, SignalResult,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn reconciliation_exports_exact_fixed_results_and_rejects_private_or_forged_names() {
    let receiver = MockServer::start().await;
    for signal in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(signal))
            .respond_with(ResponseTemplate::new(200))
            .mount(&receiver)
            .await;
    }
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .unwrap();
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.3",
            DeploymentEnvironment::Test,
            OperatingSystem::Linux,
            "test",
        )
        .unwrap(),
    )
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .unwrap()
        .unwrap()
        .handle();
    let cases = [
        (
            ProgressReconciliationOutcome::Running,
            "running",
            "deferred",
            None,
        ),
        (
            ProgressReconciliationOutcome::Settled,
            "settled",
            "ok",
            None,
        ),
        (
            ProgressReconciliationOutcome::Superseded,
            "superseded",
            "skipped",
            None,
        ),
        (
            ProgressReconciliationOutcome::NotRetained,
            "not_retained",
            "deferred",
            None,
        ),
        (
            ProgressReconciliationOutcome::Unsupported,
            "unsupported",
            "deferred",
            None,
        ),
        (
            ProgressReconciliationOutcome::Unreachable,
            "unreachable",
            "deferred",
            None,
        ),
        (
            ProgressReconciliationOutcome::Busy,
            "busy",
            "deferred",
            None,
        ),
        (
            ProgressReconciliationOutcome::AccessDenied,
            "access_denied",
            "rejected",
            None,
        ),
        (
            ProgressReconciliationOutcome::InvalidInput,
            "invalid_input",
            "rejected",
            None,
        ),
        (
            ProgressReconciliationOutcome::InvalidData,
            "invalid_data",
            "error",
            Some("corrupt"),
        ),
        (
            ProgressReconciliationOutcome::Unavailable,
            "unavailable",
            "error",
            Some("unavailable"),
        ),
        (
            ProgressReconciliationOutcome::Cancelled,
            "cancelled",
            "cancelled",
            None,
        ),
    ];
    for (result, _, _, _) in cases {
        reconciliation_span(DiagnosticRole::Local).in_scope(|| result.complete(Duration::ZERO));
    }
    for private in [
        "PRIVATE_PROGRESS_PATH",
        "clipboard.reconcile_progress.PRIVATE_DEVICE_NAME",
    ] {
        reconciliation_span(DiagnosticRole::Local).record("uc.display.name", private);
    }
    // 同样名称不能让网络能力伪装成用户本地核对。
    reconciliation_span(DiagnosticRole::Client)
        .record("uc.display.name", "clipboard.reconcile_progress.settled");
    let flush = handle.force_flush(Duration::from_secs(10));
    assert_eq!(flush.traces, SignalResult::Completed);
    assert_eq!(flush.logs, SignalResult::Completed);
    let requests = receiver.received_requests().await.unwrap();
    assert!(!requests
        .iter()
        .any(|request| request.body.windows(8).any(|bytes| bytes == b"PRIVATE_")));
    let spans = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .flat_map(|request| {
            ExportTraceServiceRequest::decode(request.body.as_slice())
                .unwrap()
                .resource_spans
        })
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .collect::<Vec<_>>();
    let logs = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/logs")
        .flat_map(|request| {
            ExportLogsServiceRequest::decode(request.body.as_slice())
                .unwrap()
                .resource_logs
        })
        .flat_map(|resource| resource.scope_logs)
        .flat_map(|scope| scope.log_records)
        .collect::<Vec<_>>();
    assert_eq!(spans.len(), cases.len());
    assert_eq!(logs.len(), cases.len());
    for (_, name, outcome, error) in cases {
        let spans = spans
            .iter()
            .filter(|span| span.name == format!("clipboard.reconcile_progress.{name}"))
            .collect::<Vec<_>>();
        assert_eq!(spans.len(), 1);
        let span = spans[0];
        assert_eq!(field(&span.attributes, "uc.record.kind"), Some("business"));
        assert_eq!(field(&span.attributes, "uc.outcome"), Some(outcome));
        assert_eq!(field(&span.attributes, "error.type"), error);
        assert!(span.parent_span_id.is_empty());
        let logs = logs
            .iter()
            .filter(|log| log.trace_id == span.trace_id && log.span_id == span.span_id)
            .collect::<Vec<_>>();
        assert_eq!(logs.len(), 1);
        assert_eq!(field(&logs[0].attributes, "uc.outcome"), Some(outcome));
        assert_eq!(field(&logs[0].attributes, "error.type"), error);
    }
}
fn reconciliation_span(role: DiagnosticRole) -> tracing::Span {
    independent_operation_span(OperationContext {
        domain: DiagnosticDomain::Clipboard,
        operation: DiagnosticOperation::ClipboardReconcileProgress,
        role,
        kind: DiagnosticSpanKind::Internal,
    })
}
fn field<'a>(fields: &'a [KeyValue], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| match field.value.as_ref()?.value.as_ref()? {
            Value::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}
