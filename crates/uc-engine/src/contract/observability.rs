//! 跨平台宿主可用的稳定观测入口。

#[cfg(target_os = "android")]
pub use uc_observability_runtime::{initialize_android_tls, AndroidTlsInitError};

pub use uc_observability_contract::analytics::{
    AdoptOutcome, AnalyticsEventContext, AnalyticsIdentityError, AnalyticsIdentityPort,
    AnalyticsPort, DeviceType, Event, GroupIdentifyPayload, IdentifyPayload, Os, ReleaseOutcome,
};
pub use uc_observability_runtime::{
    managed_log_files, ConfigError as ObservabilityConfigError, DeploymentEnvironment,
    FlushSummary as ObservabilityFlushSummary, InstallError as ObservabilityInstallError,
    InstallOutcome as ObservabilityInstallOutcome, LocalLogConfig, ObservabilityConfig,
    ObservabilityHealth, ObservabilityResource, OperatingSystem, OtlpHttpConfig,
    ProcessObservabilityHandle, ProcessObservabilityRuntime, SecretHeaderValue,
    SetupStatus as ObservabilitySetupStatus, ShutdownSummary as ObservabilityShutdownSummary,
    SignalResult as ObservabilitySignalResult, LOCAL_LOG_MAX_BYTES, LOCAL_LOG_RETENTION_DAYS,
};
