use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use napi::Status;
use uc_engine::observability::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityInstallError,
    ObservabilityInstallOutcome, ObservabilityResource, ObservabilitySetupStatus,
    ObservabilitySignalResult, OperatingSystem, OtlpHttpConfig, ProcessObservabilityHandle,
    ProcessObservabilityRuntime, SecretHeaderValue,
};
use uc_engine::HostDirectories;

use crate::{
    OhCollectorConfig, OhHostDirectories, OhObservabilityConfig, OhObservabilityHealth,
    OhObservabilitySetup, OhObservabilitySignalSummary,
};

const ENGINE_FLUSH_DEADLINE: Duration = Duration::from_millis(250);
static PROCESS_HANDLE: OnceLock<ProcessObservabilityHandle> = OnceLock::new();

pub(crate) fn install(
    config: OhObservabilityConfig,
    directories: OhHostDirectories,
) -> napi::Result<OhObservabilitySetup> {
    let directories = host_directories(directories);
    let config = runtime_config(config, &directories)?;
    let outcome = ProcessObservabilityRuntime::install(config).map_err(map_install_error)?;
    let reused = matches!(outcome, ObservabilityInstallOutcome::Reused(_));
    let handle = outcome.handle();
    let health = handle.health();
    let _ = PROCESS_HANDLE.set(handle);
    Ok(OhObservabilitySetup {
        reused,
        remote: setup_status(health.remote).to_owned(),
        local_file: setup_status(health.local_file).to_owned(),
        dropped_local_records: health.dropped_local_records as f64,
    })
}

pub(crate) fn health() -> napi::Result<OhObservabilityHealth> {
    let health = PROCESS_HANDLE.get().ok_or_else(not_installed)?.health();
    Ok(OhObservabilityHealth {
        remote: setup_status(health.remote).to_owned(),
        local_file: setup_status(health.local_file).to_owned(),
        dropped_local_records: health.dropped_local_records as f64,
        dropped_remote_spans: health.dropped_remote_spans as f64,
        dropped_remote_logs: health.dropped_remote_logs as f64,
        failed_remote_span_batches: health.failed_remote_span_batches as f64,
        failed_remote_log_batches: health.failed_remote_log_batches as f64,
    })
}

pub(crate) fn force_flush(deadline: Duration) -> napi::Result<OhObservabilitySignalSummary> {
    let handle = PROCESS_HANDLE.get().ok_or_else(not_installed)?;
    let summary = handle.force_flush(deadline);
    Ok(OhObservabilitySignalSummary {
        traces: signal_result(summary.traces).to_owned(),
        logs: signal_result(summary.logs).to_owned(),
    })
}

pub(crate) fn shutdown(deadline: Duration) -> napi::Result<OhObservabilitySignalSummary> {
    let handle = PROCESS_HANDLE.get().ok_or_else(not_installed)?;
    let summary = handle.shutdown(deadline);
    Ok(OhObservabilitySignalSummary {
        traces: signal_result(summary.traces).to_owned(),
        logs: signal_result(summary.logs).to_owned(),
    })
}

pub(crate) fn schedule_flush_after_success<T, E>(result: &Result<T, E>) {
    if result.is_err() {
        return;
    }
    let Some(handle) = PROCESS_HANDLE.get().cloned() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("uc-observability-flush".to_owned())
        .spawn(move || {
            let _ = handle.force_flush(ENGINE_FLUSH_DEADLINE);
        });
}

pub(crate) fn runtime_unavailable() -> napi::Error {
    napi::Error::new(
        Status::GenericFailure,
        "OHOS_OBSERVABILITY_RUNTIME_UNAVAILABLE".to_owned(),
    )
}

fn host_directories(directories: OhHostDirectories) -> HostDirectories {
    let cache = PathBuf::from(directories.cache_directory);
    HostDirectories::new(
        PathBuf::from(directories.private_data_directory),
        cache.clone(),
        PathBuf::from(directories.temporary_directory),
        cache.join("logs"),
    )
}

fn runtime_config(
    config: OhObservabilityConfig,
    directories: &HostDirectories,
) -> napi::Result<ObservabilityConfig> {
    let resource = ObservabilityResource::new(
        config.service_version,
        environment(&config.environment)?,
        OperatingSystem::Ohos,
        config.app_channel,
    )
    .map_err(|_| config_invalid())?;
    let runtime =
        ObservabilityConfig::new(resource).with_local_logs(LocalLogConfig::new(directories.logs()));
    if !config.remote_diagnostics_enabled {
        return Ok(runtime);
    }
    let collector = config.collector.ok_or_else(config_invalid)?;
    Ok(runtime.with_remote(remote_config(collector)?))
}

fn remote_config(config: OhCollectorConfig) -> napi::Result<OtlpHttpConfig> {
    let remote = OtlpHttpConfig::new(&config.trace_endpoint, &config.log_endpoint)
        .map_err(|_| config_invalid())?;
    match (config.auth_header_name, config.auth_header_value) {
        (Some(name), Some(value)) => remote
            .with_header(name, SecretHeaderValue::new(value))
            .map_err(|_| config_invalid()),
        (None, None) => Ok(remote),
        _ => Err(config_invalid()),
    }
}

fn environment(value: &str) -> napi::Result<DeploymentEnvironment> {
    match value {
        "development" => Ok(DeploymentEnvironment::Development),
        "test" => Ok(DeploymentEnvironment::Test),
        "staging" => Ok(DeploymentEnvironment::Staging),
        "production" => Ok(DeploymentEnvironment::Production),
        _ => Err(config_invalid()),
    }
}

fn map_install_error(error: ObservabilityInstallError) -> napi::Error {
    match error {
        ObservabilityInstallError::AlreadyInstalled => napi::Error::new(
            Status::GenericFailure,
            "OHOS_OBSERVABILITY_CONFIG_CONFLICT".to_owned(),
        ),
        ObservabilityInstallError::SubscriberAlreadyInstalled => runtime_unavailable(),
    }
}

fn config_invalid() -> napi::Error {
    napi::Error::new(
        Status::InvalidArg,
        "OHOS_OBSERVABILITY_CONFIG_INVALID".to_owned(),
    )
}

fn not_installed() -> napi::Error {
    napi::Error::new(
        Status::GenericFailure,
        "OHOS_OBSERVABILITY_NOT_INSTALLED".to_owned(),
    )
}

fn setup_status(status: ObservabilitySetupStatus) -> &'static str {
    match status {
        ObservabilitySetupStatus::Disabled => "disabled",
        ObservabilitySetupStatus::Ready => "ready",
        ObservabilitySetupStatus::Unavailable => "unavailable",
    }
}

fn signal_result(result: ObservabilitySignalResult) -> &'static str {
    match result {
        ObservabilitySignalResult::Completed => "completed",
        ObservabilitySignalResult::Failed => "failed",
        ObservabilitySignalResult::TimedOut => "timed_out",
        ObservabilitySignalResult::AlreadyShutdown => "already_shutdown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directories() -> OhHostDirectories {
        OhHostDirectories {
            private_data_directory: "private".to_owned(),
            cache_directory: "cache".to_owned(),
            temporary_directory: "temporary".to_owned(),
        }
    }

    fn config(remote_diagnostics_enabled: bool) -> OhObservabilityConfig {
        OhObservabilityConfig {
            service_version: "1.2.3".to_owned(),
            environment: "test".to_owned(),
            app_channel: "test".to_owned(),
            remote_diagnostics_enabled,
            collector: Some(OhCollectorConfig {
                trace_endpoint: "https://collector.example/v1/traces".to_owned(),
                log_endpoint: "https://collector.example/v1/logs".to_owned(),
                auth_header_name: Some("authorization".to_owned()),
                auth_header_value: Some("Bearer private-token".to_owned()),
            }),
        }
    }

    #[test]
    fn config_debug_redacts_collector_secrets() {
        let output = format!("{:?}", config(true));
        assert!(!output.contains("collector.example"));
        assert!(!output.contains("private-token"));
        assert!(!output.contains("1.2.3"));
        assert!(!output.contains("test"));
        assert!(output.contains("REDACTED"));
    }

    #[test]
    fn disabled_remote_diagnostics_ignores_collector_values() {
        let mut config = config(false);
        config.collector = Some(OhCollectorConfig {
            trace_endpoint: "invalid".to_owned(),
            log_endpoint: "invalid".to_owned(),
            auth_header_name: Some("invalid header".to_owned()),
            auth_header_value: None,
        });
        let directories = host_directories(directories());
        assert!(runtime_config(config, &directories).is_ok());
    }

    #[test]
    fn enabled_remote_diagnostics_requires_known_environment() {
        let mut config = config(true);
        config.environment = "unknown".to_owned();
        let directories = host_directories(directories());
        let error = runtime_config(config, &directories).expect_err("invalid environment");
        assert_eq!(error.reason, "OHOS_OBSERVABILITY_CONFIG_INVALID");
    }

    #[test]
    fn resource_metadata_rejects_arbitrary_host_strings() {
        for (version, channel) in [("MyPhone123", "test"), ("1.2.3", "phc_private-token")] {
            let mut config = config(false);
            config.service_version = version.to_owned();
            config.app_channel = channel.to_owned();
            let directories = host_directories(directories());
            let error = runtime_config(config, &directories).expect_err("invalid resource");
            assert_eq!(error.reason, "OHOS_OBSERVABILITY_CONFIG_INVALID");
        }
    }
}
