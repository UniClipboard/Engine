//! 跨平台宿主可用的稳定观测入口。

#[cfg(target_os = "android")]
pub use uc_observability_runtime::{initialize_android_tls, AndroidTlsInitError};

pub use uc_observability_contract::analytics::{
    AdoptOutcome, AnalyticsEventContext, AnalyticsIdentityError, AnalyticsIdentityPort,
    AnalyticsPort, DeviceType, Event, GroupIdentifyPayload, IdentifyPayload, Os, ReleaseOutcome,
};
pub use uc_observability_runtime::{
    managed_log_files, CaptureEndReason, ConfigError as ObservabilityConfigError,
    DeploymentEnvironment, DetailedCaptureRequest, FileSourceCounts,
    FlushSummary as ObservabilityFlushSummary, HostDiagnosticAction, HostDiagnosticEvent,
    HostDiagnosticFailure, HostDiagnosticOutcome, HostDiagnosticReceipt,
    HostDiagnosticRecordStatus, HostDiagnosticSource, HostLifecycleState, HostLogLayer,
    HostNetworkKind, InstallError as ObservabilityInstallError,
    InstallOutcome as ObservabilityInstallOutcome, LocalCaptureMode, LocalCaptureStatus,
    LocalDiagnosticError, LocalDiagnosticExportReport, LocalDiagnosticSource,
    LocalDiagnosticStatus, LocalLogConfig, ObservabilityConfig, ObservabilityHealth,
    ObservabilityResource, OperatingSystem, OtlpHttpConfig, ProcessObservabilityHandle,
    ProcessObservabilityRuntime, SecretHeaderValue, SetupStatus as ObservabilitySetupStatus,
    ShutdownSummary as ObservabilityShutdownSummary, SignalResult as ObservabilitySignalResult,
    SourceCapability, SourceCollection, SourceCoverage, StopCaptureResult, LOCAL_LOG_MAX_BYTES,
    LOCAL_LOG_RETENTION_DAYS,
};
