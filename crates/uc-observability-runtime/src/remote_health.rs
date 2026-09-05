use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use opentelemetry::logs::{AnyValue, LogRecord as _, Severity};
use opentelemetry::trace::{SpanKind, Status};
use opentelemetry::{Context, InstrumentationScope, Value};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{
    BatchConfigBuilder as LogBatchConfigBuilder, BatchLogProcessor, LogBatch, LogExporter,
    LogProcessor, SdkLogRecord,
};
use opentelemetry_sdk::trace::{
    BatchConfigBuilder as SpanBatchConfigBuilder, BatchSpanProcessor, Span, SpanData, SpanExporter,
    SpanProcessor,
};
use opentelemetry_sdk::Resource;

const REMOTE_QUEUE_CAPACITY: usize = 2_048;
const REMOTE_BATCH_SIZE: usize = 512;
const LOG_EVENT_NAME: &str = "uc.diagnostic";
const LOG_FIELDS: &[&str] = &[
    "event.name",
    "uc.domain",
    "uc.operation",
    "uc.role",
    "uc.outcome",
    "error.type",
    "duration_ms",
];
const SPAN_FIELDS: &[&str] = &[
    "target",
    "uc.flow.id",
    "uc.domain",
    "uc.operation",
    "uc.role",
];

#[derive(Debug, Clone, Default)]
pub(crate) struct RemoteHealthCounters {
    inner: Arc<RemoteHealthInner>,
}

#[derive(Debug, Default)]
struct RemoteHealthInner {
    dropped_spans: AtomicU64,
    dropped_logs: AtomicU64,
    failed_span_batches: AtomicU64,
    failed_log_batches: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RemoteHealthSnapshot {
    pub(crate) dropped_spans: u64,
    pub(crate) dropped_logs: u64,
    pub(crate) failed_span_batches: u64,
    pub(crate) failed_log_batches: u64,
}

impl RemoteHealthCounters {
    pub(crate) fn snapshot(&self) -> RemoteHealthSnapshot {
        RemoteHealthSnapshot {
            dropped_spans: self.inner.dropped_spans.load(Ordering::Relaxed),
            dropped_logs: self.inner.dropped_logs.load(Ordering::Relaxed),
            failed_span_batches: self.inner.failed_span_batches.load(Ordering::Relaxed),
            failed_log_batches: self.inner.failed_log_batches.load(Ordering::Relaxed),
        }
    }

    fn record_span_drop(&self, reason: DropReason) {
        if self.inner.dropped_spans.fetch_add(1, Ordering::Relaxed) == 0 {
            tracing::event!(
                target: "observability.health",
                parent: None,
                tracing::Level::WARN,
                event.name = "uc.observability.trace_dropped",
                error.type = reason.as_str(),
            );
        }
    }

    fn record_log_drop(&self, reason: DropReason) {
        if self.inner.dropped_logs.fetch_add(1, Ordering::Relaxed) == 0 {
            tracing::event!(
                target: "observability.health",
                parent: None,
                tracing::Level::WARN,
                event.name = "uc.observability.log_dropped",
                error.type = reason.as_str(),
            );
        }
    }

    fn record_span_export_failure(&self) {
        if self
            .inner
            .failed_span_batches
            .fetch_add(1, Ordering::Relaxed)
            == 0
        {
            tracing::event!(
                target: "observability.health",
                parent: None,
                tracing::Level::WARN,
                event.name = "uc.observability.trace_export_failed",
                error.type = "export_failed",
            );
        }
    }

    fn record_log_export_failure(&self) {
        if self
            .inner
            .failed_log_batches
            .fetch_add(1, Ordering::Relaxed)
            == 0
        {
            tracing::event!(
                target: "observability.health",
                parent: None,
                tracing::Level::WARN,
                event.name = "uc.observability.log_export_failed",
                error.type = "export_failed",
            );
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CapacityGate {
    state: Arc<Mutex<CapacityState>>,
    counters: RemoteHealthCounters,
    signal: Signal,
}

#[derive(Debug, Default)]
struct CapacityState {
    pending: usize,
    closed: bool,
}

#[derive(Debug, Clone, Copy)]
enum Signal {
    Spans,
    Logs,
}

#[derive(Debug, Clone, Copy)]
enum DropReason {
    QueueFull,
    Contended,
    Closed,
    SchemaRejected,
}

impl DropReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::QueueFull => "queue_full",
            Self::Contended => "queue_contended",
            Self::Closed => "runtime_closed",
            Self::SchemaRejected => "schema_rejected",
        }
    }
}

impl CapacityGate {
    fn new(counters: RemoteHealthCounters, signal: Signal) -> Self {
        Self {
            state: Arc::new(Mutex::new(CapacityState::default())),
            counters,
            signal,
        }
    }

    fn submit(&self, submit: impl FnOnce()) -> bool {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => {
                self.record_drop(DropReason::Contended);
                return false;
            }
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        if state.closed {
            drop(state);
            self.record_drop(DropReason::Closed);
            return false;
        }
        if state.pending >= REMOTE_QUEUE_CAPACITY {
            drop(state);
            self.record_drop(DropReason::QueueFull);
            return false;
        }
        state.pending += 1;
        submit();
        true
    }

    fn release(&self, count: usize) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending = state.pending.saturating_sub(count);
    }

    fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }

    fn synchronize_submissions(&self) {
        drop(
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    fn record_drop(&self, reason: DropReason) {
        match self.signal {
            Signal::Spans => self.counters.record_span_drop(reason),
            Signal::Logs => self.counters.record_log_drop(reason),
        }
    }

    fn record_rejection(&self) {
        match self.signal {
            Signal::Spans => self.counters.record_span_drop(DropReason::SchemaRejected),
            Signal::Logs => self.counters.record_log_drop(DropReason::SchemaRejected),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RemoteSubmissionControl {
    spans: CapacityGate,
    logs: CapacityGate,
}

impl RemoteSubmissionControl {
    pub(crate) fn new(counters: RemoteHealthCounters) -> Self {
        Self {
            spans: CapacityGate::new(counters.clone(), Signal::Spans),
            logs: CapacityGate::new(counters, Signal::Logs),
        }
    }

    pub(crate) fn span_gate(&self) -> CapacityGate {
        self.spans.clone()
    }

    pub(crate) fn log_gate(&self) -> CapacityGate {
        self.logs.clone()
    }

    pub(crate) fn close(&self) {
        self.spans.close();
        self.logs.close();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Barrier;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn late_request_names_are_validated_and_removed_before_export() {
        use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
        use opentelemetry::KeyValue;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

        let exporter = InMemorySpanExporter::default();
        let counters = RemoteHealthCounters::default();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(TrackedSpanProcessor::new(
                exporter.clone(),
                CapacityGate::new(counters.clone(), Signal::Spans),
            ))
            .build();
        let tracer = provider.tracer("uc-observability-runtime");
        for display in [
            "pairing.request_join.process",
            "pairing.request_join.send",
            "PRIVATE_DEVICE_NAME",
        ] {
            let mut span = tracer
                .span_builder("pairing.process_request")
                .with_kind(SpanKind::Internal)
                .with_attributes([
                    KeyValue::new("target", "uc.telemetry"),
                    KeyValue::new("uc.domain", "space_admission"),
                    KeyValue::new("uc.operation", "space_admission"),
                    KeyValue::new("uc.role", "sponsor"),
                ])
                .start(&tracer);
            span.set_attribute(KeyValue::new("uc.display.name", display));
            span.end();
        }
        provider.force_flush().expect("flush");
        let spans = exporter.get_finished_spans().expect("exported spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "pairing.request_join.process");
        assert!(spans[0]
            .attributes
            .iter()
            .all(|field| field.key.as_str() != "uc.display.name"));
        assert_eq!(counters.snapshot().dropped_spans, 2);
    }

    #[test]
    fn contended_submission_is_dropped_without_waiting() {
        let counters = RemoteHealthCounters::default();
        let gate = CapacityGate::new(counters.clone(), Signal::Spans);
        let _held = gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let started = Instant::now();
        assert!(!gate.submit(|| panic!("contended submission must not run")));

        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(counters.snapshot().dropped_spans, 1);
    }

    #[test]
    fn close_waits_for_an_accepted_submission_then_rejects_later_work() {
        let counters = RemoteHealthCounters::default();
        let gate = CapacityGate::new(counters.clone(), Signal::Logs);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let accepted = Arc::new(AtomicBool::new(false));

        let submit_gate = gate.clone();
        let submit_entered = Arc::clone(&entered);
        let submit_release = Arc::clone(&release);
        let submit_accepted = Arc::clone(&accepted);
        let submitter = std::thread::spawn(move || {
            let result = submit_gate.submit(|| {
                submit_entered.wait();
                submit_release.wait();
            });
            submit_accepted.store(result, Ordering::Release);
        });
        entered.wait();

        let close_gate = gate.clone();
        let close_finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&close_finished);
        let closer = std::thread::spawn(move || {
            close_gate.close();
            worker_finished.store(true, Ordering::Release);
        });
        std::thread::sleep(Duration::from_millis(5));
        assert!(!close_finished.load(Ordering::Acquire));

        release.wait();
        submitter.join().expect("submission thread");
        closer.join().expect("close thread");
        assert!(accepted.load(Ordering::Acquire));
        assert!(!gate.submit(|| panic!("closed gate must reject work")));
        assert_eq!(counters.snapshot().dropped_logs, 1);
    }

    #[test]
    fn flush_barrier_waits_for_an_already_accepted_submission() {
        let gate = CapacityGate::new(RemoteHealthCounters::default(), Signal::Spans);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let submit_gate = gate.clone();
        let submit_entered = Arc::clone(&entered);
        let submit_release = Arc::clone(&release);
        let submitter = std::thread::spawn(move || {
            assert!(submit_gate.submit(|| {
                submit_entered.wait();
                submit_release.wait();
            }));
        });
        entered.wait();

        let barrier_gate = gate.clone();
        let (finished_sender, finished_receiver) = std::sync::mpsc::channel();
        let barrier = std::thread::spawn(move || {
            barrier_gate.synchronize_submissions();
            finished_sender.send(()).expect("barrier result");
        });
        assert!(finished_receiver
            .recv_timeout(Duration::from_millis(5))
            .is_err());

        release.wait();
        submitter.join().expect("submission thread");
        barrier.join().expect("barrier thread");
        finished_receiver.recv().expect("barrier completed");
    }
}

#[derive(Debug)]
pub(crate) struct TrackedSpanProcessor {
    inner: BatchSpanProcessor,
    gate: CapacityGate,
}

impl TrackedSpanProcessor {
    pub(crate) fn new<E>(exporter: E, gate: CapacityGate) -> Self
    where
        E: SpanExporter + Send + 'static,
    {
        let counters = gate.counters.clone();
        let exporter = TrackedSpanExporter {
            inner: exporter,
            gate: gate.clone(),
            counters,
        };
        let config = SpanBatchConfigBuilder::default()
            .with_max_queue_size(REMOTE_QUEUE_CAPACITY)
            .with_max_export_batch_size(REMOTE_BATCH_SIZE)
            .build();
        Self {
            inner: BatchSpanProcessor::builder(exporter)
                .with_batch_config(config)
                .build(),
            gate,
        }
    }
}

impl SpanProcessor for TrackedSpanProcessor {
    fn on_start(&self, span: &mut Span, context: &Context) {
        self.inner.on_start(span, context);
    }

    fn on_end(&self, mut span: SpanData) {
        // tracing-opentelemetry 已启动的 span 忽略 otel.name 更新。
        // 流程负责人只提供固定显示名，编码前转换并移除内部字段。
        let mut display_names = span
            .attributes
            .iter()
            .filter(|field| field.key.as_str() == "uc.display.name");
        if let Some(field) = display_names.next() {
            if display_names.next().is_some() {
                self.gate.record_rejection();
                return;
            }
            let Value::String(name) = &field.value else {
                self.gate.record_rejection();
                return;
            };
            span.name = name.as_str().to_owned().into();
            span.attributes
                .retain(|field| field.key.as_str() != "uc.display.name");
        }
        if !span_is_approved(&span) {
            self.gate.record_rejection();
            return;
        }
        span.attributes
            .retain(|attribute| SPAN_FIELDS.contains(&attribute.key.as_str()));
        let _ = self.gate.submit(|| self.inner.on_end(span));
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.gate.synchronize_submissions();
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.gate.close();
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[derive(Debug)]
struct TrackedSpanExporter<E> {
    inner: E,
    gate: CapacityGate,
    counters: RemoteHealthCounters,
}

impl<E> SpanExporter for TrackedSpanExporter<E>
where
    E: SpanExporter,
{
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let count = batch.len();
        let export = self.inner.export(batch);
        let gate = self.gate.clone();
        let counters = self.counters.clone();
        async move {
            let result = export.await;
            gate.release(count);
            if result.is_err() {
                counters.record_span_export_failure();
            }
            result
        }
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.gate.synchronize_submissions();
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[derive(Debug)]
pub(crate) struct TrackedLogProcessor {
    inner: BatchLogProcessor,
    gate: CapacityGate,
}

impl TrackedLogProcessor {
    pub(crate) fn new<E>(exporter: E, gate: CapacityGate) -> Self
    where
        E: LogExporter + 'static,
    {
        let counters = gate.counters.clone();
        let exporter = TrackedLogExporter {
            inner: exporter,
            gate: gate.clone(),
            counters,
        };
        let config = LogBatchConfigBuilder::default()
            .with_max_queue_size(REMOTE_QUEUE_CAPACITY)
            .with_max_export_batch_size(REMOTE_BATCH_SIZE)
            .build();
        Self {
            inner: BatchLogProcessor::builder(exporter)
                .with_batch_config(config)
                .build(),
            gate,
        }
    }
}

impl LogProcessor for TrackedLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        let approved = data.body().is_none()
            && data.target().is_some_and(|target| target == "uc.telemetry")
            // The official tracing bridge uses an empty scope here and maps the
            // record target to the exported OTLP scope.
            && instrumentation.name().is_empty()
            && instrumentation.version().is_none()
            && instrumentation.schema_url().is_none()
            && instrumentation.attributes().next().is_none()
            && log_is_approved(data);
        if !approved {
            self.gate.record_rejection();
            return;
        }
        data.set_event_name(LOG_EVENT_NAME);
        let _ = self.gate.submit(|| self.inner.emit(data, instrumentation));
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.gate.synchronize_submissions();
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.gate.close();
        self.inner.shutdown_with_timeout(timeout)
    }

    fn event_enabled(&self, level: Severity, target: &str, name: Option<&str>) -> bool {
        self.inner.event_enabled(level, target, name)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

fn span_is_approved(span: &SpanData) -> bool {
    span_rejection_reason(span).is_none()
}

fn span_rejection_reason(span: &SpanData) -> Option<&'static str> {
    if !span.events.is_empty()
        || !span.links.is_empty()
        || !span.span_context.trace_state().header().is_empty()
        || span.instrumentation_scope.attributes().next().is_some()
        || span.instrumentation_scope.version().is_some()
        || span.instrumentation_scope.schema_url().is_some()
        || matches!(&span.status, Status::Error { description } if !description.is_empty())
    {
        return Some("span metadata");
    }

    let mut target = None;
    let mut flow = None;
    let mut domain = None;
    let mut operation = None;
    let mut role = None;
    for attribute in &span.attributes {
        let key = attribute.key.as_str();
        if !SPAN_FIELDS.contains(&key) {
            return Some("attribute key");
        }
        let Value::String(value) = &attribute.value else {
            return Some("attribute type");
        };
        let value = value.as_str();
        let slot = match key {
            "target" => &mut target,
            "uc.flow.id" => &mut flow,
            "uc.domain" => &mut domain,
            "uc.operation" => &mut operation,
            "uc.role" => &mut role,
            _ => return Some("attribute routing"),
        };
        if slot.replace(value).is_some() {
            return Some("duplicate attribute");
        }
    }

    if target != Some("uc.telemetry") {
        return Some("target");
    }
    if !flow.is_none_or(valid_flow_id) {
        return Some("flow");
    }
    if !domain.is_some_and(valid_domain) {
        return Some("domain");
    }
    if !operation.is_some_and(valid_operation) {
        return Some("operation");
    }
    if !role.is_some_and(valid_role) {
        return Some("role");
    }
    if !matches!((operation, role), (Some(operation), Some(role)) if uc_observability_contract::diagnostics::approved_operation_name(operation, role, span.name.as_ref()))
    {
        return Some("name");
    }
    if !matches!((domain, operation), (Some(domain), Some(operation)) if operation_matches_domain(domain, operation))
    {
        return Some("domain operation");
    }
    let joiner_client_flow = domain == Some("space_admission")
        && matches!(operation, Some("space_admission" | "network_transport"))
        && role == Some("joiner")
        && span.span_kind == SpanKind::Client;
    let admission_lifecycle_root = domain == Some("space_admission")
        && operation == Some("space_admission")
        && role == Some("local")
        && span.span_kind == SpanKind::Internal
        && span.parent_span_id == opentelemetry::trace::SpanId::INVALID;
    if flow.is_some() && !(joiner_client_flow || admission_lifecycle_root) {
        return Some("flow scope");
    }
    if span.span_kind == SpanKind::Internal
        && role == Some("local")
        && operation == Some("space_admission")
        && (!admission_lifecycle_root || flow.is_none())
    {
        return Some("lifecycle root");
    }
    if !matches!((operation, role), (Some(operation), Some(role)) if span_role_matches(operation, role, &span.span_kind))
    {
        return Some("operation role kind");
    }
    None
}

#[cfg(test)]
pub(crate) fn span_rejection_reason_for_test(span: &SpanData) -> Option<&'static str> {
    span_rejection_reason(span)
}

fn log_is_approved(data: &SdkLogRecord) -> bool {
    let mut event = None;
    let mut domain = None;
    let mut operation = None;
    let mut role = None;
    let mut outcome = None;
    let mut error_type = None;
    let mut duration_seen = false;

    for (key, value) in data.attributes_iter() {
        let key = key.as_str();
        if !LOG_FIELDS.contains(&key) {
            return false;
        }
        if key == "duration_ms" {
            if duration_seen || !matches!(value, AnyValue::Int(value) if *value >= 0) {
                return false;
            }
            duration_seen = true;
            continue;
        }
        let AnyValue::String(value) = value else {
            return false;
        };
        let value = value.as_str();
        let slot = match key {
            "event.name" => &mut event,
            "uc.domain" => &mut domain,
            "uc.operation" => &mut operation,
            "uc.role" => &mut role,
            "uc.outcome" => &mut outcome,
            "error.type" => &mut error_type,
            _ => return false,
        };
        if slot.replace(value).is_some() {
            return false;
        }
    }

    event == Some("uc.operation.completed")
        && domain.is_some_and(valid_domain)
        && operation.is_some_and(valid_operation)
        && role.is_some_and(valid_role)
        && outcome.is_some_and(valid_outcome)
        && duration_seen
        && matches!((domain, operation), (Some(domain), Some(operation)) if operation_matches_domain(domain, operation))
        && matches!((operation, role), (Some(operation), Some(role)) if log_role_matches(operation, role))
        && match (outcome, error_type) {
            (Some("error"), Some(error_type)) => valid_error_type(error_type),
            (Some(_), None) => true,
            _ => false,
        }
}

fn valid_flow_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_domain(value: &str) -> bool {
    matches!(
        value,
        "clipboard" | "space_admission" | "space_membership" | "storage" | "runtime"
    )
}

fn valid_operation(value: &str) -> bool {
    matches!(
        value,
        "clipboard_dispatch"
            | "clipboard_receive"
            | "clipboard_address_resolve"
            | "clipboard_connect"
            | "space_admission"
            | "membership_history_sync"
            | "membership_group_update"
            | "network_transport"
            | "profile_storage_upgrade"
            | "session_lifecycle"
    )
}

fn valid_role(value: &str) -> bool {
    matches!(
        value,
        "local" | "client" | "server" | "joiner" | "sponsor" | "member"
    )
}

fn valid_outcome(value: &str) -> bool {
    matches!(
        value,
        "ok" | "error" | "deferred" | "rejected" | "cancelled"
    )
}

fn valid_error_type(value: &str) -> bool {
    matches!(
        value,
        "authentication_failed"
            | "address_unavailable"
            | "connect_failed"
            | "stream_failed"
            | "decode_failed"
            | "timeout"
            | "channel_closed"
            | "storage"
            | "security"
            | "corrupt"
            | "source_changed"
            | "manifest"
            | "join_failed"
            | "shutdown_timeout"
            | "unavailable"
            | "peer_rejected"
            | "peer_incompatible"
            | "local_policy_exceeded"
            | "internal"
    )
}

fn operation_matches_domain(domain: &str, operation: &str) -> bool {
    matches!(
        (domain, operation),
        (
            "clipboard",
            "clipboard_dispatch"
                | "clipboard_receive"
                | "clipboard_address_resolve"
                | "clipboard_connect"
        ) | ("space_admission", "space_admission" | "network_transport")
            | (
                "space_membership",
                "membership_history_sync" | "membership_group_update"
            )
            | ("storage", "profile_storage_upgrade")
            | ("runtime", "session_lifecycle")
    )
}

fn span_role_matches(operation: &str, role: &str, kind: &SpanKind) -> bool {
    match operation {
        "clipboard_dispatch" => role == "client" && kind == &SpanKind::Client,
        "clipboard_receive" => role == "server" && kind == &SpanKind::Server,
        "clipboard_address_resolve" | "clipboard_connect" => {
            role == "client" && kind == &SpanKind::Internal
        }
        "space_admission" => {
            (role == "joiner" && kind == &SpanKind::Client)
                || (role == "local" && kind == &SpanKind::Internal)
                || (role == "sponsor" && kind == &SpanKind::Internal)
        }
        "network_transport" => {
            (role == "joiner" && kind == &SpanKind::Client)
                || (role == "sponsor" && kind == &SpanKind::Server)
        }
        "membership_history_sync" | "membership_group_update" => {
            role == "member" && matches!(kind, SpanKind::Client | SpanKind::Server)
        }
        "profile_storage_upgrade" | "session_lifecycle" => {
            role == "local" && kind == &SpanKind::Internal
        }
        _ => false,
    }
}

fn log_role_matches(operation: &str, role: &str) -> bool {
    match operation {
        "clipboard_dispatch" | "clipboard_address_resolve" | "clipboard_connect" => {
            role == "client"
        }
        "clipboard_receive" => role == "server",
        "space_admission" => matches!(role, "local" | "joiner" | "sponsor"),
        "network_transport" => matches!(role, "joiner" | "sponsor"),
        "membership_history_sync" | "membership_group_update" => {
            matches!(role, "local" | "member")
        }
        "profile_storage_upgrade" | "session_lifecycle" => role == "local",
        _ => false,
    }
}

#[derive(Debug)]
struct TrackedLogExporter<E> {
    inner: E,
    gate: CapacityGate,
    counters: RemoteHealthCounters,
}

impl<E> LogExporter for TrackedLogExporter<E>
where
    E: LogExporter,
{
    fn export(
        &self,
        batch: LogBatch<'_>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let count = batch.iter().count();
        let export = self.inner.export(batch);
        let gate = self.gate.clone();
        let counters = self.counters.clone();
        async move {
            let result = export.await;
            gate.release(count);
            if result.is_err() {
                counters.record_log_export_failure();
            }
            result
        }
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn event_enabled(&self, level: Severity, target: &str, name: Option<&str>) -> bool {
        self.inner.event_enabled(level, target, name)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}
