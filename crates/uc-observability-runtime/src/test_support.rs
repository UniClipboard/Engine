use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;

use crate::telemetry::TelemetryRuntime;

pub(crate) struct CapturedSpan {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    pub(crate) event_count: usize,
    pub(crate) flow_id: Option<String>,
    pub(crate) rejection_reason: Option<&'static str>,
}

pub(crate) struct CapturedLog {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
}

pub(crate) struct CapturedTelemetry {
    pub(crate) spans: Vec<CapturedSpan>,
    pub(crate) logs: Vec<CapturedLog>,
}

pub(crate) fn capture_telemetry(operation: impl FnOnce()) -> CapturedTelemetry {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let resource = Resource::builder_empty()
        .with_service_name("uc-engine-test")
        .build();
    let runtime = TelemetryRuntime::from_providers(
        SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .with_resource(resource.clone())
            .build(),
        SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .with_resource(resource)
            .build(),
    );
    let subscriber = tracing_subscriber::registry().with(runtime.layers());
    tracing::subscriber::with_default(subscriber, operation);
    let _ = runtime.force_flush();

    let spans = span_exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .map(|span| CapturedSpan {
            rejection_reason: crate::remote_health::span_rejection_reason_for_test(&span),
            trace_id: span.span_context.trace_id().to_string(),
            span_id: span.span_context.span_id().to_string(),
            event_count: span.events.len(),
            flow_id: span
                .attributes
                .iter()
                .find(|attribute| attribute.key.as_str() == "uc.flow.id")
                .map(|attribute| attribute.value.as_str().into_owned()),
        })
        .collect();
    let logs = log_exporter
        .get_emitted_logs()
        .unwrap_or_default()
        .into_iter()
        .map(|log| {
            let (trace_id, span_id) = log
                .record
                .trace_context()
                .map(|context| (context.trace_id.to_string(), context.span_id.to_string()))
                .unwrap_or_default();
            CapturedLog { trace_id, span_id }
        })
        .collect();
    CapturedTelemetry { spans, logs }
}
