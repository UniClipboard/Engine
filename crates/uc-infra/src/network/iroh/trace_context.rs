use std::collections::HashMap;

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use serde::{Deserialize, Serialize};
use tracing_opentelemetry::OpenTelemetrySpanExt;

const TRACEPARENT_MAX_BYTES: usize = 256;

/// 只在 Iroh 协议内部流转的有界 W3C 上下文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WireTraceContext {
    pub(super) traceparent: String,
}

impl WireTraceContext {
    pub(super) fn is_bounded(&self) -> bool {
        !self.traceparent.is_empty() && self.traceparent.len() <= TRACEPARENT_MAX_BYTES
    }
}

pub(super) fn inject_current() -> Option<WireTraceContext> {
    let mut carrier = TraceCarrier::default();
    TraceContextPropagator::new().inject_context(&tracing::Span::current().context(), &mut carrier);
    let context = WireTraceContext {
        traceparent: carrier.0.remove("traceparent")?,
    };
    context.is_bounded().then_some(context)
}

/// 仅在业务身份确认后调用，并且必须在 span 第一次进入前设置。
pub(super) fn set_remote_parent(span: &tracing::Span, wire: Option<&WireTraceContext>) -> bool {
    let Some(wire) = wire.filter(|value| value.is_bounded()) else {
        return false;
    };
    let carrier = TraceCarrier(HashMap::from([(
        "traceparent".to_owned(),
        wire.traceparent.clone(),
    )]));
    let context = TraceContextPropagator::new()
        .extract_with_context(&opentelemetry::Context::new(), &carrier);
    if !context.span().span_context().is_valid() || !context.span().span_context().is_remote() {
        return false;
    }
    span.set_parent(context).is_ok()
}

#[derive(Default)]
struct TraceCarrier(HashMap<String, String>);

impl Injector for TraceCarrier {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
}

impl Extractor for TraceCarrier {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn valid_context_round_trips_as_a_remote_parent() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("trace-context-test");
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_context_activation(true),
        );

        tracing::subscriber::with_default(subscriber, || {
            let client = tracing::info_span!("client");
            let _client_entered = client.enter();
            let wire = inject_current().expect("current context");
            let server = tracing::info_span!("server");
            assert!(set_remote_parent(&server, Some(&wire)));
            let _server_entered = server.enter();
        });
        provider.force_flush().expect("flush");

        let spans = exporter.get_finished_spans().expect("spans");
        let client = spans
            .iter()
            .find(|span| span.name == "client")
            .expect("client");
        let server = spans
            .iter()
            .find(|span| span.name == "server")
            .expect("server");
        assert_eq!(
            server.span_context.trace_id(),
            client.span_context.trace_id()
        );
        assert_eq!(server.parent_span_id, client.span_context.span_id());
    }

    #[test]
    fn malformed_and_oversized_contexts_are_ignored() {
        let malformed = WireTraceContext {
            traceparent: "not-a-traceparent".to_owned(),
        };
        let oversized = WireTraceContext {
            traceparent: "x".repeat(TRACEPARENT_MAX_BYTES + 1),
        };
        assert!(!set_remote_parent(
            &tracing::info_span!("malformed"),
            Some(&malformed)
        ));
        assert!(!set_remote_parent(
            &tracing::info_span!("oversized"),
            Some(&oversized)
        ));
        assert!(!set_remote_parent(&tracing::info_span!("missing"), None));
    }
}
