use std::any::TypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing_subscriber::filter::{dynamic_filter_fn, filter_fn};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use uc_observability_contract::diagnostics::TELEMETRY_SCHEMA_VERSION;

use crate::config::{ObservabilityConfig, OtlpHttpConfig};
use crate::filter::{remote_span_enabled, sdk_log_enabled};
use crate::local_file::LocalFileRuntime;
use crate::local_log_processor::LocalLogProcessor;
use crate::local_recording::LocalRecordingState;
use crate::remote_health::{
    RemoteHealthCounters, RemoteHealthSnapshot, RemoteSubmissionControl, TracingSpans,
    TrackedLogProcessor, TrackedSpanProcessor,
};
use crate::status::{RemoteSetupFailure, SetupStatus, SignalResult};
use crate::subscriber::RuntimeLayer;
use crate::subscriber_gate::SubscriberGate;
#[cfg(test)]
use crate::{DeploymentEnvironment, ObservabilityResource, OperatingSystem};

#[derive(Clone)]
pub(crate) struct TelemetryRuntime {
    pub(crate) recording: Arc<LocalRecordingState>,
    traces: SdkTracerProvider,
    logs: SdkLoggerProvider,
    health: RemoteHealthCounters,
    submission: Option<RemoteSubmissionControl>,
    accepting: Arc<AtomicBool>,
}

pub(crate) struct TelemetrySignalSummary {
    pub(crate) traces: SignalResult,
    pub(crate) logs: SignalResult,
    pub(crate) trace_error: Option<Arc<opentelemetry_sdk::error::OTelSdkError>>,
    pub(crate) log_error: Option<Arc<opentelemetry_sdk::error::OTelSdkError>>,
}

struct GuardedTraceLayer<L> {
    inner: L,
    activity: Option<Arc<TracingSpans>>,
}

struct TraceBoundaryLayer {
    activity: Arc<TracingSpans>,
    begins: bool,
}

impl Layer<Registry> for TraceBoundaryLayer {
    fn on_new_span(
        &self,
        _: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        _: Context<'_, Registry>,
    ) {
        if self.begins {
            self.activity.start(id);
        }
    }

    fn on_close(&self, id: tracing::span::Id, _: Context<'_, Registry>) {
        if !self.begins {
            self.activity.end(&id);
        }
    }
}

impl<L: Layer<Registry> + 'static> Layer<Registry> for GuardedTraceLayer<L> {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        context: Context<'_, Registry>,
    ) {
        if self.admitted(id) {
            self.inner.on_new_span(attrs, id, context);
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        context: Context<'_, Registry>,
    ) {
        if self.admitted(id) {
            self.inner.on_record(id, values, context);
        }
    }

    fn on_follows_from(
        &self,
        id: &tracing::span::Id,
        follows: &tracing::span::Id,
        context: Context<'_, Registry>,
    ) {
        if self.admitted(id) {
            self.inner.on_follows_from(id, follows, context);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, context: Context<'_, Registry>) {
        self.inner.on_event(event, context);
    }

    fn on_enter(&self, id: &tracing::span::Id, context: Context<'_, Registry>) {
        if self.admitted(id) {
            self.inner.on_enter(id, context);
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, context: Context<'_, Registry>) {
        if self.admitted(id) {
            self.inner.on_exit(id, context);
        }
    }

    fn on_close(&self, id: tracing::span::Id, context: Context<'_, Registry>) {
        if self.admitted(&id) {
            self.inner.on_close(id.clone(), context);
        }
    }

    fn on_id_change(
        &self,
        old: &tracing::span::Id,
        new: &tracing::span::Id,
        context: Context<'_, Registry>,
    ) {
        if self
            .activity
            .as_ref()
            .is_none_or(|activity| activity.replace(old, new))
        {
            self.inner.on_id_change(old, new, context);
        }
    }

    // SAFETY: 指针由被包装的层提供，借用期不超过该层及 self 的借用期。
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            Some(self as *const _ as *const ())
        } else {
            // SAFETY: 保留调用方的类型与生命周期约束，原样转交内层。
            unsafe { self.inner.downcast_raw(id) }
        }
    }
}

impl<L> GuardedTraceLayer<L> {
    fn admitted(&self, id: &tracing::span::Id) -> bool {
        self.activity
            .as_ref()
            .is_none_or(|activity| activity.contains(id))
    }
}

impl TelemetryRuntime {
    pub(crate) fn guard_subscriber<S>(&self, subscriber: S) -> SubscriberGate<S> {
        SubscriberGate::new(
            subscriber,
            self.submission
                .as_ref()
                .map(RemoteSubmissionControl::tracing_activity),
        )
    }

    pub(crate) fn trace_boundary(&self, begins: bool) -> Option<RuntimeLayer> {
        self.submission.as_ref().map(|submission| {
            Box::new(
                TraceBoundaryLayer {
                    activity: submission.tracing_activity(),
                    begins,
                }
                .with_filter(filter_fn(remote_span_enabled)),
            ) as RuntimeLayer
        })
    }

    pub(crate) fn new(
        config: &ObservabilityConfig,
        local_file: Option<Arc<LocalFileRuntime>>,
    ) -> (Self, RemoteSetup) {
        Self::new_with(config, local_file, Self::remote)
    }

    fn new_with(
        config: &ObservabilityConfig,
        local_file: Option<Arc<LocalFileRuntime>>,
        build_remote: impl FnOnce(
            Resource,
            &OtlpHttpConfig,
            Arc<AtomicBool>,
            Option<Arc<LocalFileRuntime>>,
            Arc<LocalRecordingState>,
        ) -> Result<Self, RemoteSetupFailure>,
    ) -> (Self, RemoteSetup) {
        let resource = resource(config);
        let accepting = Arc::new(AtomicBool::new(true));
        let recording = Arc::new(LocalRecordingState::new(
            &config.resource,
            local_file.as_ref(),
        ));
        let Some(remote) = config.remote.as_ref() else {
            let runtime = Self::local(resource, accepting, local_file, recording);
            return (runtime, RemoteSetup::status(SetupStatus::Disabled));
        };
        match build_remote(
            resource.clone(),
            remote,
            Arc::clone(&accepting),
            local_file.clone(),
            Arc::clone(&recording),
        ) {
            Ok(runtime) => (runtime, RemoteSetup::status(SetupStatus::Ready)),
            Err(failure) => (
                Self::local(resource, accepting, local_file, recording),
                RemoteSetup {
                    status: SetupStatus::Unavailable,
                    failure: Some(failure),
                },
            ),
        }
    }

    pub(crate) fn layers(&self) -> Vec<RuntimeLayer> {
        let trace_accepting = Arc::clone(&self.accepting);
        let tracer = self.traces.tracer("uc-observability-runtime");
        let trace_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_context_activation(true)
            .with_location(false)
            .with_threads(false)
            .with_tracked_inactivity(false);
        let trace_layer = GuardedTraceLayer {
            inner: trace_layer,
            activity: self
                .submission
                .as_ref()
                .map(RemoteSubmissionControl::tracing_activity),
        }
        .with_filter(dynamic_filter_fn(move |metadata, _| {
            trace_accepting.load(Ordering::Acquire) && remote_span_enabled(metadata)
        }));
        let log_accepting = Arc::clone(&self.accepting);
        let log_layer = OpenTelemetryTracingBridge::new(&self.logs).with_filter(dynamic_filter_fn(
            move |metadata, _| log_accepting.load(Ordering::Acquire) && sdk_log_enabled(metadata),
        ));
        vec![Box::new(trace_layer), Box::new(log_layer)]
    }

    pub(crate) fn accepting(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.accepting)
    }

    pub(crate) fn seal(&self) {
        self.accepting.store(false, Ordering::Release);
        if let Some(submission) = &self.submission {
            submission.close();
        }
    }

    pub(crate) fn pause(&self) {
        self.accepting.store(false, Ordering::Release);
        if let Some(submission) = &self.submission {
            submission.pause();
        }
    }

    pub(crate) fn close_admission(&self) {
        self.accepting.store(false, Ordering::Release);
        if let Some(submission) = &self.submission {
            submission.close_span_admission();
        }
    }

    pub(crate) fn wait_accepted_spans(&self, interrupted: &impl Fn() -> bool) -> Option<bool> {
        self.submission
            .as_ref()
            .map_or(Some(true), |submission| submission.wait_spans(interrupted))
    }

    pub(crate) fn notify_span_wait(&self) {
        if let Some(submission) = &self.submission {
            submission.notify_span_wait();
        }
    }

    pub(crate) fn resume(&self) -> bool {
        if self
            .submission
            .as_ref()
            .is_some_and(|submission| !submission.resume())
        {
            return false;
        }
        self.accepting.store(true, Ordering::Release);
        true
    }

    pub(crate) fn force_flush(&self) -> TelemetrySignalSummary {
        let (traces, trace_error) = signal_with_error(self.traces.force_flush());
        let (logs, log_error) = signal_with_error(self.logs.force_flush());
        TelemetrySignalSummary {
            traces,
            logs,
            trace_error,
            log_error,
        }
    }

    pub(crate) fn shutdown(&self, deadline: Duration) -> TelemetrySignalSummary {
        let (traces, trace_error) = signal_with_error(self.traces.shutdown_with_timeout(deadline));
        let (logs, log_error) = signal_with_error(self.logs.shutdown_with_timeout(deadline));
        TelemetrySignalSummary {
            traces,
            logs,
            trace_error,
            log_error,
        }
    }

    pub(crate) fn health(&self) -> RemoteHealthSnapshot {
        self.health.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn from_providers(traces: SdkTracerProvider, logs: SdkLoggerProvider) -> Self {
        Self {
            recording: Arc::new(LocalRecordingState::new(
                &ObservabilityResource::new(
                    "1.0.0",
                    DeploymentEnvironment::Test,
                    OperatingSystem::Other,
                    "test",
                )
                .expect("test resource"),
                None,
            )),
            traces,
            logs,
            health: RemoteHealthCounters::default(),
            submission: None,
            accepting: Arc::new(AtomicBool::new(true)),
        }
    }

    fn local(
        resource: Resource,
        accepting: Arc<AtomicBool>,
        local_file: Option<Arc<LocalFileRuntime>>,
        recording: Arc<LocalRecordingState>,
    ) -> Self {
        Self {
            traces: SdkTracerProvider::builder()
                .with_resource(resource.clone())
                .build(),
            logs: local_logger_provider(resource, local_file, recording.clone()).build(),
            recording,
            health: RemoteHealthCounters::default(),
            submission: None,
            accepting,
        }
    }

    fn remote(
        resource: Resource,
        config: &OtlpHttpConfig,
        accepting: Arc<AtomicBool>,
        local_file: Option<Arc<LocalFileRuntime>>,
        recording: Arc<LocalRecordingState>,
    ) -> Result<Self, RemoteSetupFailure> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let headers = config
            .headers()
            .iter()
            .map(|(name, value)| (name.clone(), value.expose().to_owned()))
            .collect::<HashMap<_, _>>();
        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout())
            .build()
            // discarded-source[observability-init]: `reqwest::Error`: setup failure only degrades to a fixed status and the log channel is not built yet
            .map_err(|_| RemoteSetupFailure::HttpClient)?;
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_http_client(client.clone())
            .with_endpoint(config.trace_endpoint())
            .with_timeout(config.timeout())
            .with_protocol(Protocol::HttpBinary)
            .with_headers(headers.clone())
            .build()
            // discarded-source[observability-init]: `opentelemetry_otlp::ExporterBuildError`: setup failure only degrades to a fixed status and the log channel is not built yet
            .map_err(|_| RemoteSetupFailure::TraceExporter)?;
        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_http_client(client)
            .with_endpoint(config.log_endpoint())
            .with_timeout(config.timeout())
            .with_protocol(Protocol::HttpBinary)
            .with_headers(headers)
            .build()
            // discarded-source[observability-init]: `opentelemetry_otlp::ExporterBuildError`: setup failure only degrades to a fixed status and the log channel is not built yet
            .map_err(|_| RemoteSetupFailure::LogExporter)?;
        let health = RemoteHealthCounters::default();
        let submission = RemoteSubmissionControl::new(health.clone());
        Ok(Self {
            traces: SdkTracerProvider::builder()
                .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOn)))
                .with_span_processor(TrackedSpanProcessor::new(
                    span_exporter,
                    submission.span_gate(),
                    submission.span_activity(),
                ))
                .with_resource(resource.clone())
                .build(),
            logs: local_logger_provider(resource, local_file, recording.clone())
                .with_log_processor(TrackedLogProcessor::new(
                    log_exporter,
                    submission.log_gate(),
                ))
                .build(),
            health,
            recording,
            submission: Some(submission),
            accepting,
        })
    }
}

fn local_logger_provider(
    resource: Resource,
    local_file: Option<Arc<LocalFileRuntime>>,
    recording: Arc<LocalRecordingState>,
) -> opentelemetry_sdk::logs::LoggerProviderBuilder {
    let builder = SdkLoggerProvider::builder().with_resource(resource);
    match local_file {
        Some(file) => builder.with_log_processor(LocalLogProcessor::new(file, recording)),
        None => builder,
    }
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

#[cfg(test)]
fn signal_result<T>(result: Result<T, opentelemetry_sdk::error::OTelSdkError>) -> SignalResult {
    match result {
        Ok(_) => SignalResult::Completed,
        Err(opentelemetry_sdk::error::OTelSdkError::AlreadyShutdown) => {
            SignalResult::AlreadyShutdown
        }
        Err(opentelemetry_sdk::error::OTelSdkError::Timeout(_)) => SignalResult::TimedOut,
        Err(_) => SignalResult::Failed,
    }
}

fn signal_with_error(
    result: Result<(), opentelemetry_sdk::error::OTelSdkError>,
) -> (
    SignalResult,
    Option<Arc<opentelemetry_sdk::error::OTelSdkError>>,
) {
    let classification = match &result {
        Ok(()) => SignalResult::Completed,
        Err(opentelemetry_sdk::error::OTelSdkError::AlreadyShutdown) => {
            SignalResult::AlreadyShutdown
        }
        Err(opentelemetry_sdk::error::OTelSdkError::Timeout(_)) => SignalResult::TimedOut,
        Err(_) => SignalResult::Failed,
    };
    (classification, result.err().map(Arc::new))
}

/// 远端导出器装配结果：状态加上失败阶段（仅失败时存在）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct RemoteSetup {
    pub(crate) status: SetupStatus,
    pub(crate) failure: Option<RemoteSetupFailure>,
}

impl RemoteSetup {
    fn status(status: SetupStatus) -> Self {
        Self {
            status,
            failure: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_timeout_is_reported_as_timed_out() {
        let result = signal_result::<()>(Err(opentelemetry_sdk::error::OTelSdkError::Timeout(
            Duration::from_millis(1),
        )));

        assert_eq!(result, SignalResult::TimedOut);
    }
    use crate::{
        DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    };

    fn config_with_remote() -> ObservabilityConfig {
        let remote = OtlpHttpConfig::new_loopback(
            "http://127.0.0.1:4318/v1/traces",
            "http://127.0.0.1:4318/v1/logs",
        )
        .expect("远端配置");
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("资源配置"),
        )
        .with_remote(remote)
    }

    #[test]
    fn a_failed_remote_build_degrades_to_local_and_keeps_the_failing_stage() {
        for failure in [
            RemoteSetupFailure::HttpClient,
            RemoteSetupFailure::TraceExporter,
            RemoteSetupFailure::LogExporter,
        ] {
            let (_runtime, setup) =
                TelemetryRuntime::new_with(&config_with_remote(), None, |_, _, _, _, _| {
                    Err(failure)
                });

            assert_eq!(setup.status, SetupStatus::Unavailable);
            assert_eq!(setup.failure, Some(failure));
        }
    }

    #[test]
    fn without_remote_configuration_there_is_no_failure_stage() {
        let config = ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("资源配置"),
        );

        let (_runtime, setup) = TelemetryRuntime::new(&config, None);

        assert_eq!(setup.status, SetupStatus::Disabled);
        assert_eq!(setup.failure, None);
    }
}
