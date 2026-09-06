use std::time::{Duration, Instant};

use tracing::Instrument;
use uc_core::{TaskRegistry, TaskShutdownReport};
use uc_observability_contract::diagnostics::{
    complete_operation, complete_unassociated_operation, operation_span, record_task_shutdown,
    DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationCompletion, OperationContext,
};

pub(super) async fn shutdown_tasks(tasks: &TaskRegistry, deadline: Duration) -> TaskShutdownReport {
    let started = Instant::now();
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Runtime,
        operation: DiagnosticOperation::TaskShutdown,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    let report = tasks.shutdown(deadline).instrument(span.clone()).await;
    record_task_shutdown(
        report.completed_count,
        report.timed_out_count,
        report.join_error_count,
    );
    let completion = if report.timed_out_count > 0 {
        OperationCompletion::failed(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::TaskShutdown,
            DiagnosticRole::Local,
            DiagnosticErrorType::ShutdownTimeout,
            started.elapsed(),
        )
    } else if report.join_error_count > 0 {
        OperationCompletion::failed(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::TaskShutdown,
            DiagnosticRole::Local,
            DiagnosticErrorType::JoinFailed,
            started.elapsed(),
        )
    } else {
        OperationCompletion::succeeded(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::TaskShutdown,
            DiagnosticRole::Local,
            started.elapsed(),
        )
    };
    if report.timed_out_count == 0 && report.join_error_count == 0 {
        // 正常清理不建立独立业务记录，日志也不指向即将省略的节点。
        span.record("uc.outcome", "ok");
        span.record("otel.status_code", "OK");
        complete_unassociated_operation(completion);
    } else {
        span.in_scope(|| complete_operation(completion));
    }
    report
}

#[cfg(all(test, feature = "dev-tools"))]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::{
        logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest,
    };
    use opentelemetry_proto::tonic::common::v1::{any_value::Value, KeyValue};
    use prost::Message;
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
    #[ignore = "独占进程观测，精确运行"]
    async fn normal_cleanup_is_log_only_but_failures_keep_traces() {
        let receiver = MockServer::start().await;
        for endpoint in ["/v1/traces", "/v1/logs"] {
            Mock::given(method("POST"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(200))
                .mount(&receiver)
                .await;
        }
        assert!(crate::init_test_tracing_with_otlp(
            &format!("{}/v1/traces", receiver.uri()),
            &format!("{}/v1/logs", receiver.uri())
        ));
        let normal = TaskRegistry::new();
        assert!(
            normal
                .spawn(|cancel| async move {
                    cancel.cancelled().await;
                })
                .await
        );
        let report = shutdown_tasks(&normal, Duration::from_secs(1)).await;
        assert_eq!(report.completed_count, 1);
        assert_eq!(report.timed_out_count, 0);
        let slow = TaskRegistry::new();
        assert!(slow.spawn(|_| std::future::pending()).await);
        assert_eq!(
            shutdown_tasks(&slow, Duration::from_millis(10))
                .await
                .timed_out_count,
            1
        );
        let panicked = TaskRegistry::new();
        assert!(
            panicked
                .spawn(|_| async { panic!("intentional test panic") })
                .await
        );
        assert_eq!(
            shutdown_tasks(&panicked, Duration::from_secs(1))
                .await
                .join_error_count,
            1
        );
        crate::flush_test_tracing();
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
        assert_eq!(spans.len(), 2, "normal cleanup has no standalone trace");
        for reason in ["shutdown_timeout", "join_failed"] {
            let span = spans
                .iter()
                .find(|span| field(&span.attributes, "error.type") == Some(reason))
                .expect("error retained");
            assert_eq!(span.name, "runtime.shutdown_tasks");
            assert_eq!(
                field(&span.attributes, "uc.record.kind"),
                Some("diagnostic")
            );
            assert!(logs
                .iter()
                .any(|log| log.span_id == span.span_id && log.trace_id == span.trace_id));
        }
        let normal = logs
            .iter()
            .find(|log| field(&log.attributes, "uc.outcome") == Some("ok"))
            .expect("normal log retained");
        assert!(normal.trace_id.is_empty() && normal.span_id.is_empty());
    }
}
