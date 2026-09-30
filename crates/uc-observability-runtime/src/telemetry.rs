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
use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::TELEMETRY_SCHEMA_VERSION;

use crate::config::{ObservabilityConfig, OtlpHttpConfig};
use crate::filter::{remote_span_enabled, sdk_log_enabled};
use crate::local_file::LocalFileRuntime;
use crate::local_log_processor::LocalLogProcessor;
use crate::local_recording::LocalRecordingState;
use crate::remote_health::{
    RemoteHealthCounters, RemoteHealthSnapshot, RemoteSubmissionControl, TrackedLogProcessor,
    TrackedSpanProcessor,
};
use crate::status::{RemoteSetupFailure, SetupStatus, SignalResult};
use crate::subscriber::RuntimeLayer;
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
}

impl TelemetryRuntime {
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
            .with_tracked_inactivity(false)
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

    pub(crate) fn force_flush(&self) -> TelemetrySignalSummary {
        TelemetrySignalSummary {
            traces: signal_result(self.traces.force_flush()),
            logs: signal_result(self.logs.force_flush()),
        }
    }

    pub(crate) fn shutdown(&self, deadline: Duration) -> TelemetrySignalSummary {
        TelemetrySignalSummary {
            traces: signal_result(self.traces.shutdown_with_timeout(deadline)),
            logs: signal_result(self.logs.shutdown_with_timeout(deadline)),
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
            // 此时日志通道尚未建立；只保留固定阶段分类，由安装完成后的 health 事件与 `ObservabilityHealth` 报告，不携带底层错误正文。
            .map_err(|_| RemoteSetupFailure::HttpClient)?;
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_http_client(client.clone())
            .with_endpoint(config.trace_endpoint())
            .with_timeout(config.timeout())
            .with_protocol(Protocol::HttpBinary)
            .with_headers(headers.clone())
            .build()
            // 此时日志通道尚未建立；只保留固定阶段分类，由安装完成后的 health 事件与 `ObservabilityHealth` 报告，不携带底层错误正文。
            .map_err(|_| RemoteSetupFailure::TraceExporter)?;
        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_http_client(client)
            .with_endpoint(config.log_endpoint())
            .with_timeout(config.timeout())
            .with_protocol(Protocol::HttpBinary)
            .with_headers(headers)
            .build()
            // 此时日志通道尚未建立；只保留固定阶段分类，由安装完成后的 health 事件与 `ObservabilityHealth` 报告，不携带底层错误正文。
            .map_err(|_| RemoteSetupFailure::LogExporter)?;
        let health = RemoteHealthCounters::default();
        let submission = RemoteSubmissionControl::new(health.clone());
        Ok(Self {
            traces: SdkTracerProvider::builder()
                .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOn)))
                .with_span_processor(TrackedSpanProcessor::new(
                    span_exporter,
                    submission.span_gate(),
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
