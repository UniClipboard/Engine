//! 跨平台宿主可用的稳定观测入口。

use crate::HostDirectories;

#[cfg(target_os = "android")]
pub use uc_observability_runtime::{initialize_android_tls, AndroidTlsInitError};

pub use uc_observability_contract::analytics::{
    AdoptOutcome, AnalyticsEventContext, AnalyticsIdentityError, AnalyticsIdentityPort,
    AnalyticsPort, DeviceType, Event, GroupIdentifyPayload, IdentifyPayload, Os, ReleaseOutcome,
};
pub use uc_observability_contract::error_source::io_error_kind;
pub use uc_observability_contract::log_fields::{log_id, log_vocab, log_vocab_debug};
pub use uc_observability_contract::{
    analytics, diagnostics, uc_debug, uc_error, uc_info, uc_trace, uc_warn,
};
pub use uc_observability_runtime::{
    managed_log_files, migrate_managed_log_files, CaptureEndReason,
    ConfigError as ObservabilityConfigError, DeploymentEnvironment, DetailedCaptureRequest,
    FileSourceCounts, FlushSummary as ObservabilityFlushSummary, HostDiagnosticAction,
    HostDiagnosticEvent, HostDiagnosticFailure, HostDiagnosticOutcome, HostDiagnosticReceipt,
    HostDiagnosticRecordStatus, HostDiagnosticSource, HostLifecycleState, HostLogLayer,
    HostNetworkKind, InstallError as ObservabilityInstallError,
    InstallOutcome as ObservabilityInstallOutcome, LocalCaptureMode, LocalCaptureStatus,
    LocalDiagnosticError, LocalDiagnosticExportReport, LocalDiagnosticSource,
    LocalDiagnosticStatus, LocalLogConfig, ModuleLogCounts, NetworkCaptureMode,
    NetworkCaptureStatus, NetworkLibraryStatus, NetworkLogBudget, NetworkLogConfig,
    NetworkLogCounters, NetworkLogError, NetworkLogExportReport, NetworkLogFile, NetworkLogState,
    NetworkLogStatus, NetworkProcessRole, ObservabilityConfig, ObservabilityHealth,
    ObservabilityResource, OperatingSystem, OtlpHttpConfig, ProcessObservabilityHandle,
    ProcessObservabilityRuntime, RemoteSetupFailure as ObservabilityRemoteSetupFailure,
    SecretHeaderValue, SetupStatus as ObservabilitySetupStatus,
    ShutdownSummary as ObservabilityShutdownSummary, SignalResult as ObservabilitySignalResult,
    SourceCapability, SourceCollection, SourceCoverage, StopCaptureResult, LOCAL_LOG_MAX_BYTES,
    LOCAL_LOG_RETENTION_DAYS, NETWORK_LOG_SCHEMA_VERSION,
};

/// 产品使用的全部 ALPN，用于登记独立网络日志允许输出的协议取值（见 `NetworkLogConfig::with_known_alpns`）。
pub fn known_network_alpns() -> Vec<String> {
    uc_infra_p2p::network::iroh::product_alpns()
        .into_iter()
        .map(|alpn| String::from_utf8_lossy(alpn).into_owned())
        .collect()
}

/// 旧版移动端布局把日志放在缓存目录内；安装观测运行期之前把它们迁入当前日志目录（一次性，幂等）。
///
/// 迁移失败不影响安装：旧文件留在原处，由 [`LegacyLogMigration::record`] 在运行期安装后留下一条固定分类记录。
pub fn migrate_legacy_mobile_logs(directories: &HostDirectories) -> LegacyLogMigration {
    match migrate_managed_log_files(&directories.legacy_mobile_logs(), directories.logs()) {
        Ok(0) => LegacyLogMigration::NotNeeded,
        Ok(moved) => LegacyLogMigration::Moved(moved),
        Err(error) => LegacyLogMigration::Failed(error),
    }
}

#[derive(Debug)]
pub enum LegacyLogMigration {
    NotNeeded,
    Moved(usize),
    Failed(std::io::Error),
}

impl LegacyLogMigration {
    /// 运行期安装后写入迁移结果；无需迁移时不记录。
    pub fn record(self) {
        match self {
            Self::NotNeeded => {}
            Self::Moved(count) => uc_info!(
                target: "uc_engine::observability",
                outcome = "moved",
                count = count,
                "legacy mobile logs moved out of the cache"
            ),
            Self::Failed(error) => uc_warn!(
                target: "uc_engine::observability",
                outcome = "failed",
                error_kind = "legacy_log_migration",
                io_error_kind = io_error_kind(&error),
                "legacy mobile logs could not be moved out of the cache"
            ),
        }
    }
}
