use std::sync::OnceLock;
use std::time::Duration;

use uc_engine::observability::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityInstallError,
    ObservabilityInstallOutcome, ObservabilityResource, ObservabilitySetupStatus,
    ObservabilitySignalResult, OperatingSystem, OtlpHttpConfig, ProcessObservabilityHandle,
    ProcessObservabilityRuntime, SecretHeaderValue,
};
use uc_engine::HostDirectories;

use crate::{
    BindingCollectorConfig, BindingDeploymentEnvironment, BindingError, BindingObservabilityConfig,
    BindingObservabilityFlushSummary, BindingObservabilitySetup, BindingObservabilitySetupStatus,
    BindingObservabilityShutdownSummary, BindingObservabilitySignalResult,
};

const ENGINE_FLUSH_DEADLINE: Duration = Duration::from_millis(250);
static PROCESS_HANDLE: OnceLock<ProcessObservabilityHandle> = OnceLock::new();

pub(crate) fn install(
    config: BindingObservabilityConfig,
    directories: &HostDirectories,
) -> Result<BindingObservabilitySetup, BindingError> {
    let config = runtime_config(config, directories)?;
    let outcome = ProcessObservabilityRuntime::install(config).map_err(map_install_error)?;
    let reused = matches!(outcome, ObservabilityInstallOutcome::Reused(_));
    let handle = outcome.handle();
    let health = handle.health();
    let _ = PROCESS_HANDLE.set(handle);
    Ok(BindingObservabilitySetup {
        reused,
        remote: map_setup_status(health.remote),
        local_file: map_setup_status(health.local_file),
        dropped_local_records: health.dropped_local_records,
    })
}

pub(crate) fn force_flush(
    deadline: Duration,
) -> Result<BindingObservabilityFlushSummary, BindingError> {
    let handle = PROCESS_HANDLE
        .get()
        .ok_or(BindingError::ObservabilityNotInstalled)?;
    let summary = handle.force_flush(deadline);
    Ok(BindingObservabilityFlushSummary {
        traces: map_signal_result(summary.traces),
        logs: map_signal_result(summary.logs),
    })
}

pub(crate) fn shutdown(
    deadline: Duration,
) -> Result<BindingObservabilityShutdownSummary, BindingError> {
    let handle = PROCESS_HANDLE
        .get()
        .ok_or(BindingError::ObservabilityNotInstalled)?;
    let summary = handle.shutdown(deadline);
    Ok(BindingObservabilityShutdownSummary {
        traces: map_signal_result(summary.traces),
        logs: map_signal_result(summary.logs),
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

fn runtime_config(
    config: BindingObservabilityConfig,
    directories: &HostDirectories,
) -> Result<ObservabilityConfig, BindingError> {
    let resource = ObservabilityResource::new(
        config.service_version,
        map_environment(config.environment),
        operating_system(),
        std::env::consts::ARCH,
        config.app_channel,
    );
    let runtime =
        ObservabilityConfig::new(resource).with_local_logs(LocalLogConfig::new(directories.logs()));
    if !config.remote_diagnostics_enabled {
        return Ok(runtime);
    }
    #[cfg(target_os = "android")]
    crate::android::ensure_android_context_installed()?;
    let collector = config
        .collector
        .ok_or(BindingError::ObservabilityConfigInvalid)?;
    Ok(runtime.with_remote(remote_config(collector)?))
}

fn remote_config(config: BindingCollectorConfig) -> Result<OtlpHttpConfig, BindingError> {
    let remote = OtlpHttpConfig::new(&config.trace_endpoint, &config.log_endpoint)
        .map_err(|_| BindingError::ObservabilityConfigInvalid)?;
    match (config.auth_header_name, config.auth_header_value) {
        (Some(name), Some(value)) => remote
            .with_header(name, SecretHeaderValue::new(value))
            .map_err(|_| BindingError::ObservabilityConfigInvalid),
        (None, None) => Ok(remote),
        _ => Err(BindingError::ObservabilityConfigInvalid),
    }
}

fn map_environment(environment: BindingDeploymentEnvironment) -> DeploymentEnvironment {
    match environment {
        BindingDeploymentEnvironment::Development => DeploymentEnvironment::Development,
        BindingDeploymentEnvironment::Test => DeploymentEnvironment::Test,
        BindingDeploymentEnvironment::Staging => DeploymentEnvironment::Staging,
        BindingDeploymentEnvironment::Production => DeploymentEnvironment::Production,
    }
}

fn operating_system() -> OperatingSystem {
    match std::env::consts::OS {
        "ios" => OperatingSystem::Ios,
        "android" => OperatingSystem::Android,
        "macos" => OperatingSystem::Macos,
        "windows" => OperatingSystem::Windows,
        "linux" => OperatingSystem::Linux,
        _ => OperatingSystem::Other,
    }
}

fn map_install_error(error: ObservabilityInstallError) -> BindingError {
    match error {
        ObservabilityInstallError::AlreadyInstalled => BindingError::ObservabilityConfigConflict,
        ObservabilityInstallError::SubscriberAlreadyInstalled => {
            BindingError::ObservabilityRuntimeUnavailable
        }
    }
}

fn map_setup_status(status: ObservabilitySetupStatus) -> BindingObservabilitySetupStatus {
    match status {
        ObservabilitySetupStatus::Disabled => BindingObservabilitySetupStatus::Disabled,
        ObservabilitySetupStatus::Ready => BindingObservabilitySetupStatus::Ready,
        ObservabilitySetupStatus::Unavailable => BindingObservabilitySetupStatus::Unavailable,
    }
}

fn map_signal_result(result: ObservabilitySignalResult) -> BindingObservabilitySignalResult {
    match result {
        ObservabilitySignalResult::Completed => BindingObservabilitySignalResult::Completed,
        ObservabilitySignalResult::Failed => BindingObservabilitySignalResult::Failed,
        ObservabilitySignalResult::TimedOut => BindingObservabilitySignalResult::TimedOut,
        ObservabilitySignalResult::AlreadyShutdown => {
            BindingObservabilitySignalResult::AlreadyShutdown
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;

    fn directories() -> HostDirectories {
        HostDirectories::new(
            PathBuf::from("private"),
            PathBuf::from("cache"),
            PathBuf::from("temporary"),
            PathBuf::from("cache/logs"),
        )
    }

    fn config(remote_diagnostics_enabled: bool) -> BindingObservabilityConfig {
        BindingObservabilityConfig {
            service_version: "1.2.3".to_owned(),
            environment: BindingDeploymentEnvironment::Test,
            app_channel: "test".to_owned(),
            remote_diagnostics_enabled,
            collector: Some(BindingCollectorConfig {
                trace_endpoint: "https://collector.example/v1/traces".to_owned(),
                log_endpoint: "https://collector.example/v1/logs".to_owned(),
                auth_header_name: Some("authorization".to_owned()),
                auth_header_value: Some("Bearer private-token".to_owned()),
            }),
        }
    }

    #[test]
    fn binding_config_debug_redacts_collector_secrets() {
        let output = format!("{:?}", config(true));
        assert!(!output.contains("collector.example"));
        assert!(!output.contains("private-token"));
        assert!(output.contains("REDACTED"));
    }

    #[test]
    fn disabled_remote_diagnostics_does_not_parse_collector_configuration() {
        let mut config = config(false);
        config.collector = Some(BindingCollectorConfig {
            trace_endpoint: "not an endpoint".to_owned(),
            log_endpoint: "not an endpoint".to_owned(),
            auth_header_name: Some("invalid header".to_owned()),
            auth_header_value: None,
        });
        assert!(runtime_config(config, &directories()).is_ok());
    }

    #[test]
    fn enabled_remote_diagnostics_requires_complete_auth_header_pair() {
        let mut config = config(true);
        config
            .collector
            .as_mut()
            .expect("collector")
            .auth_header_value = None;
        assert!(matches!(
            runtime_config(config, &directories()),
            Err(BindingError::ObservabilityConfigInvalid)
        ));
    }

    #[test]
    fn scheduled_flush_keeps_one_host_log_across_suspend_and_resume() {
        let root = tempfile::tempdir().expect("temporary directory");
        let directories = HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("cache/logs"),
        );
        let mut config = config(false);
        config.collector = None;
        let setup = install(config, &directories).expect("process runtime install");
        assert_eq!(setup.remote, BindingObservabilitySetupStatus::Disabled);
        assert_eq!(setup.local_file, BindingObservabilitySetupStatus::Ready);

        emit_test_completion("storage", "profile_storage_upgrade");
        schedule_flush_after_success(&Ok::<(), ()>(()));
        let first = wait_for_log(&directories, "profile_storage_upgrade");
        assert!(first.contains("profile_storage_upgrade"));

        emit_test_completion("runtime", "session_lifecycle");
        schedule_flush_after_success(&Ok::<(), ()>(()));
        let resumed = wait_for_log(&directories, "session_lifecycle");
        assert!(resumed.contains("profile_storage_upgrade"));
        assert!(resumed.contains("session_lifecycle"));

        let alive = force_flush(Duration::from_millis(250)).expect("process flush");
        assert_ne!(
            alive.logs,
            BindingObservabilitySignalResult::AlreadyShutdown
        );
        let _ = shutdown(Duration::from_millis(250)).expect("process shutdown");
        let closed = force_flush(Duration::from_millis(10)).expect("closed summary");
        assert_eq!(
            closed.logs,
            BindingObservabilitySignalResult::AlreadyShutdown
        );
    }

    fn emit_test_completion(domain: &'static str, operation: &'static str) {
        tracing::event!(
            target: "uc.telemetry",
            tracing::Level::INFO,
            event.name = "uc.operation.completed",
            uc.domain = domain,
            uc.operation = operation,
            uc.role = "local",
            uc.outcome = "ok",
            duration_ms = 1_u64,
        );
    }

    fn wait_for_log(directories: &HostDirectories, expected: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let content = uc_engine::observability::managed_log_files(directories.logs())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|path| std::fs::read_to_string(path).ok())
                .collect::<String>();
            if content.contains(expected) || Instant::now() >= deadline {
                return content;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
