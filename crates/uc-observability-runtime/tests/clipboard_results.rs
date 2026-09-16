use std::time::Duration;

use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{any_value::Value, KeyValue};
use prost::Message;
use uc_observability_contract::diagnostics::*;
use uc_observability_runtime::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn field<'a>(fields: &'a [KeyValue], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| match field.value.as_ref()?.value.as_ref()? {
            Value::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_results_survive_real_export_with_matching_logs() {
    let receiver = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&receiver)
            .await;
    }
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("endpoints");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("worker")
        .expect("install")
        .handle();
    let domain = DiagnosticDomain::Clipboard;
    let operation = DiagnosticOperation::ClipboardCopyAndSync;
    let role = DiagnosticRole::Local;
    for completion in [
        OperationCompletion::partial(domain, operation, role, Duration::ZERO),
        OperationCompletion::skipped(domain, operation, role, Duration::ZERO),
        OperationCompletion::deferred(domain, operation, role, Duration::ZERO),
        OperationCompletion::failed(
            domain,
            operation,
            role,
            DiagnosticErrorType::DeliveryFailed,
            Duration::ZERO,
        ),
    ] {
        operation_span(OperationContext {
            domain,
            operation,
            role,
            kind: DiagnosticSpanKind::Internal,
        })
        .in_scope(|| complete_operation(completion));
    }
    let flush = handle.force_flush(Duration::from_secs(10));
    assert_eq!(flush.traces, SignalResult::Completed);
    assert_eq!(flush.logs, SignalResult::Completed);
    let requests = receiver.received_requests().await.expect("requests");
    let spans = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/traces")
        .flat_map(|r| {
            ExportTraceServiceRequest::decode(r.body.as_slice())
                .expect("traces")
                .resource_spans
        })
        .flat_map(|r| r.scope_spans)
        .flat_map(|s| s.spans)
        .collect::<Vec<_>>();
    let logs = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/logs")
        .flat_map(|r| {
            ExportLogsServiceRequest::decode(r.body.as_slice())
                .expect("logs")
                .resource_logs
        })
        .flat_map(|r| r.scope_logs)
        .flat_map(|s| s.log_records)
        .collect::<Vec<_>>();
    assert_eq!(spans.len(), 4);
    assert_eq!(logs.len(), 4);
    for expected in ["partial", "skipped", "deferred", "error"] {
        let span = spans
            .iter()
            .find(|s| field(&s.attributes, "uc.outcome") == Some(expected))
            .expect("outcome exported");
        let log = logs
            .iter()
            .find(|l| l.trace_id == span.trace_id && l.span_id == span.span_id)
            .expect("matching log");
        assert_eq!(field(&log.attributes, "uc.outcome"), Some(expected));
        assert_eq!(
            field(&span.attributes, "error.type"),
            if expected == "error" {
                Some("delivery_failed")
            } else {
                None
            }
        );
        assert_eq!(
            field(&log.attributes, "error.type"),
            field(&span.attributes, "error.type")
        );
        assert_eq!(
            span.status.as_ref().map(|s| s.code),
            Some(if expected == "error" { 2 } else { 0 })
        );
    }
    // 显式验收时只转发上述合成记录到本机，不接受外部目标配置。
    if std::env::var_os("UC_DIAGNOSTIC_RESULTS_JAEGER").is_some() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for request in requests {
            let mut child = Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "10",
                    "-H",
                    "Content-Type: application/x-protobuf",
                    "--data-binary",
                    "@-",
                ])
                .arg(format!("http://127.0.0.1:4318{}", request.url.path()))
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .expect("collector client");
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(&request.body)
                .expect("body");
            assert!(child.wait().expect("reply").success());
        }
        for span in spans {
            println!(
                "{} {}",
                field(&span.attributes, "uc.outcome").expect("outcome"),
                span.trace_id
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
        }
    }
}
