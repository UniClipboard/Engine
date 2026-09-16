use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, record_task_join_failure, record_task_shutdown,
    DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind, DiagnosticTaskKind,
    OperationCompletion, OperationContext,
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

    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("fixture endpoints")
    .with_header(
        "authorization",
        SecretHeaderValue::new("Bearer PRIVATE_OTLP_TOKEN"),
    )
    .expect("fixture header");
    assert!(ObservabilityResource::new(
        "PRIVATE/VERSION",
        DeploymentEnvironment::Test,
        OperatingSystem::Macos,
        "PRIVATE CHANNEL TOKEN",
    )
    .is_err());
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.3",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("approved resource"),
    );
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
    let conflicting = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.4",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("approved conflicting resource"),
    );
    assert!(matches!(
        ProcessObservabilityRuntime::install(conflicting),
        Err(InstallError::AlreadyInstalled)
    ));

    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    {
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            Duration::from_millis(9),
        ));
        uc_observability_contract::diagnostics::complete_unassociated_operation(
            OperationCompletion::succeeded(
                DiagnosticDomain::Runtime,
                DiagnosticOperation::TaskShutdown,
                DiagnosticRole::Local,
                Duration::ZERO,
            ),
        );
        tracing::event!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            path = "/private/sensitive-path",
            "PRIVATE_APPROVED_TARGET_BODY"
        );
        tracing::event!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            event.name = "uc.operation.completed",
            "PRIVATE_BODY_WITH_ALLOWLISTED_FIELDS"
        );
        tracing::warn!(
            target: "uc_application::clipboard",
            path = "/private/sensitive-path",
            "unapproved event"
        );
    }
    drop(span);
    let forged_business = tracing::span!(target: "uc.telemetry", tracing::Level::INFO,
        "uc.operation", otel.name = "runtime.shutdown_tasks", otel.kind = "internal",
        uc.domain = "runtime", uc.operation = "runtime.shutdown_tasks", uc.role = "local",
        uc.outcome = "error", error.type = "shutdown_timeout", uc.record.kind = "business");
    drop(forged_business);
    // 新准入字段仍拒绝自由文本、缺失分类及互相矛盾的结果。
    for (outcome, error) in [
        ("PRIVATE_SPAN_RESULT", "decode_failed"),
        ("error", "PRIVATE_SPAN_ERROR"),
        ("ok", "decode_failed"),
    ] {
        let invalid = operation_span(OperationContext {
            domain: DiagnosticDomain::Storage,
            operation: DiagnosticOperation::ProfileStorageUpgrade,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        });
        invalid.record("uc.outcome", outcome);
        invalid.record("error.type", error);
    }
    let private_value_span = tracing::span!(
        target: "uc.telemetry",
        tracing::Level::INFO,
        "uc.operation",
        otel.name = "phc_private-span",
        uc.domain = "storage",
        uc.operation = "phc_private-span",
        uc.role = "local",
        otel.kind = "internal",
        otel.status_code = tracing::field::Empty,
    );
    drop(private_value_span);
    tracing::event!(
        target: "uc.telemetry",
        tracing::Level::INFO,
        event.name = "uc.operation.completed",
        uc.domain = "storage",
        uc.operation = "profile_storage_upgrade",
        uc.role = "MyPhone123",
        uc.outcome = "ok",
        duration_ms = 1_u64,
    );
    tracing::event!(
        target: "uc.telemetry",
        tracing::Level::INFO,
        event.name = "uc.operation.completed",
        uc.domain = "storage",
        uc.operation = "profile_storage_upgrade",
        uc.role = "client",
        uc.outcome = "ok",
        duration_ms = 1_u64,
    );
    let private_parent = tracing::info_span!(
        target: "uc_application::clipboard",
        "private_parent",
        device_id = "PRIVATE_PARENT_DEVICE",
        path = "/private/parent-path",
    );
    {
        let _entered = private_parent.enter();
        record_task_join_failure(DiagnosticTaskKind::ClipboardDeferredDrain);
    }
    drop(private_parent);
    record_task_shutdown(3, 1, 2);

    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(1)),
        SignalResult::Completed
    );
    let current_files = managed_log_files(log_directory.path()).expect("current managed logs");
    let current_jsonl = std::fs::read_to_string(&current_files[0]).expect("current JSONL output");
    assert!(current_jsonl.contains("uc.task.join_failed"));
    assert!(current_jsonl.contains("uc.task.shutdown"));
    assert!(current_jsonl.contains("task.timed_out.count"));
    assert!(!current_jsonl.contains("PRIVATE_PARENT_DEVICE"));
    assert!(!current_jsonl.contains("/private/parent-path"));

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
    let resource = traces.resource_spans[0]
        .resource
        .as_ref()
        .expect("trace resource");
    assert_eq!(
        attribute_keys(&resource.attributes),
        [
            "deployment.environment.name",
            "host.arch",
            "os.type",
            "service.instance.id",
            "service.name",
            "service.namespace",
            "service.version",
            "uc.app.channel",
            "uc.telemetry.schema.version",
        ]
    );
    assert_eq!(
        traces.resource_spans[0].scope_spans[0]
            .scope
            .as_ref()
            .map(|scope| scope.name.as_str()),
        Some("uc-observability-runtime")
    );
    assert_eq!(
        logs.resource_logs[0].scope_logs[0]
            .scope
            .as_ref()
            .map(|scope| scope.name.as_str()),
        Some("uc.telemetry")
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
    assert_eq!(records.len(), 2);
    let unassociated = records.iter().find(|record| record.attributes.iter().any(|field| field.key == "uc.operation" && matches!(field.value.as_ref().and_then(|value| value.value.as_ref()), Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value)) if value == "runtime.shutdown_tasks"))).expect("unassociated log");
    assert!(
        unassociated.trace_id.is_empty() && unassociated.span_id.is_empty(),
        "unassociated logs must not borrow the active outer operation"
    );
    assert_eq!(spans[0].name, "profile_storage_upgrade");
    assert!(
        spans[0]
            .attributes
            .iter()
            .any(|field| field.key == "uc.record.kind"),
        "business and runtime records must be independently searchable"
    );
    assert!(
        spans[0]
            .attributes
            .iter()
            .any(|field| field.key == "uc.outcome"),
        "trace page must show the operation result without a separate log backend"
    );
    assert!(spans[0].events.is_empty());
    assert_eq!(spans[0].status.as_ref().map(|status| status.code), Some(1));
    assert_eq!(
        attribute_keys(&spans[0].attributes),
        [
            "target",
            "uc.domain",
            "uc.operation",
            "uc.outcome",
            "uc.record.kind",
            "uc.role",
        ]
    );
    assert_eq!(
        attribute_keys(&records[0].attributes),
        [
            "duration_ms",
            "event.name",
            "uc.domain",
            "uc.operation",
            "uc.outcome",
            "uc.role",
        ]
    );
    assert_eq!(records[0].event_name, "uc.diagnostic");
    assert!(records[0].body.is_none());
    assert_eq!(records[0].trace_id, spans[0].trace_id);
    assert_eq!(records[0].span_id, spans[0].span_id);

    for request in [trace_request, log_request] {
        let payload = String::from_utf8_lossy(&request.body);
        assert!(!payload.contains("PRIVATE_OTLP_TOKEN"));
        assert!(!payload.contains("/private/sensitive-path"));
        assert!(!payload.contains("PRIVATE_APPROVED_TARGET_BODY"));
        assert!(!payload.contains("PRIVATE_BODY_WITH_ALLOWLISTED_FIELDS"));
        assert!(!payload.contains("PRIVATE_PARENT_DEVICE"));
        assert!(!payload.contains("/private/parent-path"));
        assert!(!payload.contains("PRIVATE/VERSION"));
        assert!(!payload.contains("/private/device-arch"));
        assert!(!payload.contains("PRIVATE CHANNEL TOKEN"));
        assert!(!payload.contains("private-device-name"));
        assert!(!payload.contains("phc_private-span"));
        assert!(!payload.contains("MyPhone123"));
    }

    let files = managed_log_files(log_directory.path()).expect("managed log files");
    assert_eq!(files.len(), 1);
    let jsonl = std::fs::read_to_string(&files[0]).expect("JSONL output");
    assert!(jsonl.contains("uc.operation.completed"));
    assert!(jsonl.contains("uc.task.join_failed"));
    assert!(!jsonl.contains("/private/sensitive-path"));
    assert!(!jsonl.contains("PRIVATE_APPROVED_TARGET_BODY"));
    assert!(!jsonl.contains("phc_private-span"));
    assert!(!jsonl.contains("MyPhone123"));
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

fn attribute_keys(attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue]) -> Vec<&str> {
    let mut keys = attributes
        .iter()
        .map(|attribute| attribute.key.as_str())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}
