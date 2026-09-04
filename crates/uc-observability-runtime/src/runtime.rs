use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::{TELEMETRY_SCHEMA_VERSION, TELEMETRY_TARGET};

use crate::config::{ObservabilityConfig, OtlpHttpConfig};
use crate::local_file::BoundedDailyMakeWriter;

type RuntimeLayer = Box<dyn Layer<Registry> + Send + Sync>;

static INSTALL_GUARD: Mutex<()> = Mutex::new(());
static INSTALLED: OnceLock<Arc<RuntimeState>> = OnceLock::new();

pub struct ProcessObservabilityRuntime;

impl ProcessObservabilityRuntime {
    pub fn install(config: ObservabilityConfig) -> Result<InstallOutcome, InstallError> {
        let _install_guard = INSTALL_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(installed) = INSTALLED.get() {
            if installed.config == config {
                return Ok(InstallOutcome::Reused(ProcessObservabilityHandle {
                    state: Arc::clone(installed),
                }));
            }
            return Err(InstallError::AlreadyInstalled);
        }

        let (state, subscriber) = build_runtime(config);
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|_| InstallError::SubscriberAlreadyInstalled)?;
        let _ = tracing_log::LogTracer::init();
        INSTALLED
            .set(Arc::clone(&state))
            .map_err(|_| InstallError::AlreadyInstalled)?;
        Ok(InstallOutcome::Installed(ProcessObservabilityHandle {
            state,
        }))
    }
}

pub enum InstallOutcome {
    Installed(ProcessObservabilityHandle),
    Reused(ProcessObservabilityHandle),
}

impl InstallOutcome {
    pub fn handle(&self) -> ProcessObservabilityHandle {
        match self {
            Self::Installed(handle) | Self::Reused(handle) => handle.clone(),
        }
    }
}

impl fmt::Debug for InstallOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed(_) => formatter.write_str("Installed(REDACTED)"),
            Self::Reused(_) => formatter.write_str("Reused(REDACTED)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("observability runtime is already installed with a different configuration")]
    AlreadyInstalled,
    #[error("the process already has a tracing subscriber")]
    SubscriberAlreadyInstalled,
}

#[derive(Clone)]
pub struct ProcessObservabilityHandle {
    state: Arc<RuntimeState>,
}

impl fmt::Debug for ProcessObservabilityHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProcessObservabilityHandle(REDACTED)")
    }
}

impl ProcessObservabilityHandle {
    pub fn health(&self) -> ObservabilityHealth {
        let mut health = self.state.health;
        health.dropped_local_records = self.state.dropped_local_records.load(Ordering::Relaxed);
        health
    }

    pub fn force_flush(&self, deadline: Duration) -> FlushSummary {
        if self.state.shutdown_started.load(Ordering::Acquire) {
            return FlushSummary::already_shutdown();
        }
        let providers = self.state.providers.clone();
        let file_writer = self.state.file_writer.clone();
        run_with_deadline(deadline, move || FlushSummary {
            traces: signal_result(providers.traces.force_flush()),
            logs: flush_logs(&providers.logs, file_writer),
        })
        .unwrap_or_else(FlushSummary::timed_out)
    }

    pub fn shutdown(&self, deadline: Duration) -> ShutdownSummary {
        if self.state.shutdown_started.swap(true, Ordering::AcqRel) {
            return ShutdownSummary::already_shutdown();
        }
        let providers = self.state.providers.clone();
        let file_writer = self.state.file_writer.clone();
        run_with_deadline(deadline, move || ShutdownSummary {
            traces: signal_result(providers.traces.shutdown_with_timeout(deadline)),
            logs: shutdown_logs(&providers.logs, file_writer, deadline),
        })
        .unwrap_or_else(ShutdownSummary::timed_out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStatus {
    Disabled,
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservabilityHealth {
    pub remote: SetupStatus,
    pub local_file: SetupStatus,
    pub dropped_local_records: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalResult {
    Completed,
    Failed,
    TimedOut,
    AlreadyShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushSummary {
    pub traces: SignalResult,
    pub logs: SignalResult,
}

impl FlushSummary {
    fn timed_out() -> Self {
        Self {
            traces: SignalResult::TimedOut,
            logs: SignalResult::TimedOut,
        }
    }

    fn already_shutdown() -> Self {
        Self {
            traces: SignalResult::AlreadyShutdown,
            logs: SignalResult::AlreadyShutdown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownSummary {
    pub traces: SignalResult,
    pub logs: SignalResult,
}

impl ShutdownSummary {
    fn timed_out() -> Self {
        Self {
            traces: SignalResult::TimedOut,
            logs: SignalResult::TimedOut,
        }
    }

    fn already_shutdown() -> Self {
        Self {
            traces: SignalResult::AlreadyShutdown,
            logs: SignalResult::AlreadyShutdown,
        }
    }
}

struct RuntimeState {
    config: ObservabilityConfig,
    providers: ProviderPair,
    health: ObservabilityHealth,
    dropped_local_records: Arc<AtomicU64>,
    file_writer: Option<tracing_appender::non_blocking::NonBlocking>,
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
    shutdown_started: AtomicBool,
}

#[derive(Clone)]
struct ProviderPair {
    traces: SdkTracerProvider,
    logs: SdkLoggerProvider,
}

fn build_runtime(
    config: ObservabilityConfig,
) -> (Arc<RuntimeState>, impl tracing::Subscriber + Send + Sync) {
    let resource = resource(&config);
    let (providers, remote) = match config
        .remote
        .as_ref()
        .and_then(|remote| remote_providers(resource.clone(), remote).ok())
    {
        Some(providers) => (providers, SetupStatus::Ready),
        None if config.remote.is_some() => (local_providers(resource), SetupStatus::Unavailable),
        None => (local_providers(resource), SetupStatus::Disabled),
    };

    let mut layers = telemetry_layers(&providers);
    layers.push(system_layer());
    let dropped_local_records = Arc::new(AtomicU64::new(0));
    let (local_file, file_writer, file_guard, dropped_local_records) =
        match config.local_logs.as_ref() {
            Some(local) => match local_file_layer(&local.directory) {
                Ok((layer, writer, guard, dropped)) => {
                    layers.push(layer);
                    (SetupStatus::Ready, Some(writer), Some(guard), dropped)
                }
                Err(()) => (SetupStatus::Unavailable, None, None, dropped_local_records),
            },
            None => (SetupStatus::Disabled, None, None, dropped_local_records),
        };

    let subscriber = tracing_subscriber::registry().with(layers);
    let state = Arc::new(RuntimeState {
        config,
        providers,
        health: ObservabilityHealth {
            remote,
            local_file,
            dropped_local_records: 0,
        },
        dropped_local_records,
        file_writer,
        _file_guard: file_guard,
        shutdown_started: AtomicBool::new(false),
    });
    (state, subscriber)
}

fn resource(config: &ObservabilityConfig) -> Resource {
    Resource::builder_empty()
        .with_service_name("uc-engine")
        .with_attributes([
            KeyValue::new("service.namespace", "uniclipboard"),
            KeyValue::new("service.version", config.resource.service_version.clone()),
            KeyValue::new("service.instance.id", uuid::Uuid::new_v4().to_string()),
            KeyValue::new(
                "deployment.environment.name",
                config.resource.environment.as_str(),
            ),
            KeyValue::new("os.type", config.resource.os.as_str()),
            KeyValue::new("host.arch", config.resource.arch.clone()),
            KeyValue::new("uc.app.channel", config.resource.app_channel.clone()),
            KeyValue::new(
                "uc.telemetry.schema.version",
                i64::from(TELEMETRY_SCHEMA_VERSION),
            ),
        ])
        .build()
}

fn local_providers(resource: Resource) -> ProviderPair {
    ProviderPair {
        traces: SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .build(),
        logs: SdkLoggerProvider::builder().with_resource(resource).build(),
    }
}

fn remote_providers(resource: Resource, config: &OtlpHttpConfig) -> Result<ProviderPair, ()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let headers = config
        .headers()
        .iter()
        .map(|(name, value)| (name.clone(), value.expose().to_owned()))
        .collect::<HashMap<_, _>>();
    let client = reqwest::blocking::Client::builder()
        .timeout(config.timeout())
        .build()
        .map_err(|_| ())?;
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_http_client(client.clone())
        .with_endpoint(config.trace_endpoint())
        .with_timeout(config.timeout())
        .with_protocol(Protocol::HttpBinary)
        .with_headers(headers.clone())
        .build()
        .map_err(|_| ())?;
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_http_client(client)
        .with_endpoint(config.log_endpoint())
        .with_timeout(config.timeout())
        .with_protocol(Protocol::HttpBinary)
        .with_headers(headers)
        .build()
        .map_err(|_| ())?;
    Ok(ProviderPair {
        traces: SdkTracerProvider::builder()
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOn)))
            .with_batch_exporter(span_exporter)
            .with_resource(resource.clone())
            .build(),
        logs: SdkLoggerProvider::builder()
            .with_batch_exporter(log_exporter)
            .with_resource(resource)
            .build(),
    })
}

fn telemetry_layers(providers: &ProviderPair) -> Vec<RuntimeLayer> {
    let tracer = providers.traces.tracer("uc-observability-runtime");
    let trace_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_context_activation(true)
        .with_filter(filter_fn(|metadata| {
            metadata.is_span() && metadata.target() == TELEMETRY_TARGET
        }));
    let log_layer =
        OpenTelemetryTracingBridge::new(&providers.logs).with_filter(filter_fn(|metadata| {
            metadata.is_event() && metadata.target() == TELEMETRY_TARGET
        }));
    vec![Box::new(trace_layer), Box::new(log_layer)]
}

fn local_file_layer(
    directory: &std::path::Path,
) -> Result<
    (
        RuntimeLayer,
        tracing_appender::non_blocking::NonBlocking,
        tracing_appender::non_blocking::WorkerGuard,
        Arc<AtomicU64>,
    ),
    (),
> {
    let (writer, dropped) = BoundedDailyMakeWriter::new(directory).map_err(|_| ())?;
    let (writer, guard) = tracing_appender::non_blocking(writer);
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(writer.clone())
        .with_filter(filter_fn(local_sink_enabled));
    Ok((Box::new(layer), writer, guard, dropped))
}

#[cfg(target_vendor = "apple")]
fn system_layer() -> RuntimeLayer {
    Box::new(
        tracing_oslog::OsLogger::new("app.uniclipboard", "engine")
            .with_filter(filter_fn(local_sink_enabled)),
    )
}

#[cfg(target_os = "android")]
fn system_layer() -> RuntimeLayer {
    match tracing_android::layer("UcEngine") {
        Ok(layer) => Box::new(layer.with_filter(filter_fn(local_sink_enabled))),
        Err(_) => Box::new(tracing_subscriber::layer::Identity::new()),
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "android")))]
fn system_layer() -> RuntimeLayer {
    Box::new(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_filter(filter_fn(local_sink_enabled)),
    )
}

fn local_sink_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    matches!(
        *metadata.level(),
        tracing::Level::ERROR | tracing::Level::WARN | tracing::Level::INFO
    ) && matches!(
        metadata.target(),
        "uc.telemetry"
            | "observability.health"
            | "admission.performance"
            | "membership.performance"
            | "storage.performance"
    )
}

fn signal_result<T>(result: Result<T, opentelemetry_sdk::error::OTelSdkError>) -> SignalResult {
    match result {
        Ok(_) => SignalResult::Completed,
        Err(opentelemetry_sdk::error::OTelSdkError::AlreadyShutdown) => {
            SignalResult::AlreadyShutdown
        }
        Err(_) => SignalResult::Failed,
    }
}

fn flush_logs(
    provider: &SdkLoggerProvider,
    file_writer: Option<tracing_appender::non_blocking::NonBlocking>,
) -> SignalResult {
    let provider_result = signal_result(provider.force_flush());
    let file_result = file_writer.map_or(Ok(()), |mut writer| writer.flush());
    if file_result.is_err() && provider_result == SignalResult::Completed {
        SignalResult::Failed
    } else {
        provider_result
    }
}

fn shutdown_logs(
    provider: &SdkLoggerProvider,
    file_writer: Option<tracing_appender::non_blocking::NonBlocking>,
    deadline: Duration,
) -> SignalResult {
    let file_result = file_writer.map_or(Ok(()), |mut writer| writer.flush());
    let provider_result = signal_result(provider.shutdown_with_timeout(deadline));
    if file_result.is_err() && provider_result == SignalResult::Completed {
        SignalResult::Failed
    } else {
        provider_result
    }
}

fn run_with_deadline<T: Send + 'static>(
    deadline: Duration,
    operation: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let _ = std::thread::Builder::new()
        .name("uc-observability-lifecycle".to_owned())
        .spawn(move || {
            let _ = sender.send(operation());
        });
    receiver.recv_timeout(deadline).ok()
}

#[cfg(test)]
pub(crate) struct CapturedSpan {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    pub(crate) event_count: usize,
}

#[cfg(test)]
pub(crate) struct CapturedLog {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
}

#[cfg(test)]
pub(crate) struct CapturedTelemetry {
    pub(crate) spans: Vec<CapturedSpan>,
    pub(crate) logs: Vec<CapturedLog>,
}

#[cfg(test)]
pub(crate) fn capture_telemetry(operation: impl FnOnce()) -> CapturedTelemetry {
    use opentelemetry_sdk::logs::InMemoryLogExporter;
    use opentelemetry_sdk::trace::InMemorySpanExporter;

    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let resource = Resource::builder_empty()
        .with_service_name("uc-engine-test")
        .build();
    let providers = ProviderPair {
        traces: SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .with_resource(resource.clone())
            .build(),
        logs: SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .with_resource(resource)
            .build(),
    };
    let layers = telemetry_layers(&providers);
    let subscriber = tracing_subscriber::registry().with(layers);
    tracing::subscriber::with_default(subscriber, operation);
    let _ = providers.traces.force_flush();
    let _ = providers.logs.force_flush();

    let spans = span_exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .map(|span| CapturedSpan {
            trace_id: span.span_context.trace_id().to_string(),
            span_id: span.span_context.span_id().to_string(),
            event_count: span.events.len(),
        })
        .collect();
    let logs = log_exporter
        .get_emitted_logs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|log| {
            log.record.trace_context().map(|context| CapturedLog {
                trace_id: context.trace_id.to_string(),
                span_id: context.span_id.to_string(),
            })
        })
        .collect();
    CapturedTelemetry { spans, logs }
}
