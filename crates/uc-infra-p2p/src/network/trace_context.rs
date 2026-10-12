use std::collections::HashMap;

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uc_sync_protocol::WireTraceContext;

pub(crate) fn inject_current() -> Option<WireTraceContext> {
    let mut carrier = TraceCarrier::default();
    TraceContextPropagator::new().inject_context(&tracing::Span::current().context(), &mut carrier);
    let context = WireTraceContext {
        traceparent: carrier.0.remove("traceparent")?,
    };
    context.is_bounded().then_some(context)
}

/// 仅在业务身份确认后调用，并且必须在 span 第一次进入前设置。
pub(crate) fn set_remote_parent(span: &tracing::Span, wire: Option<&WireTraceContext>) -> bool {
    let Some(context) = authenticated_parent(wire) else {
        return false;
    };
    span.set_parent(context).is_ok()
}

pub(crate) fn authenticated_parent(
    wire: Option<&WireTraceContext>,
) -> Option<opentelemetry::Context> {
    let wire = wire.filter(|value| value.is_bounded())?;
    let carrier = TraceCarrier(HashMap::from([(
        "traceparent".to_owned(),
        wire.traceparent.clone(),
    )]));
    let context = TraceContextPropagator::new()
        .extract_with_context(&opentelemetry::Context::new(), &carrier);
    (context.span().span_context().is_valid() && context.span().span_context().is_remote())
        .then_some(context)
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
    use uc_sync_protocol::TRACEPARENT_MAX_BYTES;

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
    fn authenticated_request_binds_pending_receive_before_first_enter() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("pending-inbound-test"))
                .with_context_activation(true),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tracing::subscriber::with_default(subscriber, || {
            runtime.block_on(async {
                let client = tracing::info_span!("pending-client");
                let wire = client.in_scope(|| inject_current().unwrap());
                let server = tracing::info_span!(parent: None, "pending-server");
                uc_observability_contract::diagnostics::scope_pending_inbound(server, async {
                    let parent = authenticated_parent(Some(&wire)).unwrap();
                    assert!(
                        uc_observability_contract::diagnostics::bind_authenticated_inbound_parent(
                            parent
                        )
                    );
                    uc_observability_contract::diagnostics::scope_authenticated_inbound(async {
                        let _observation =
                            uc_observability_contract::diagnostics::capture_inbound_observation();
                    })
                    .await;
                })
                .await;
            })
        });
        provider.force_flush().unwrap();
        let spans = exporter.get_finished_spans().unwrap();
        let client = spans
            .iter()
            .find(|span| span.name == "pending-client")
            .unwrap();
        let server = spans
            .iter()
            .find(|span| span.name == "pending-server")
            .unwrap();
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
