//! 第三方网络库独立日志的附加绑定；原安装配置、业务入口和错误布局保持兼容。
//!
//! 宿主在 `install_process_observability` 之前调用 [`configure_network_log`] 登记角色与容量预算，
//! 未登记则整条链路不存在。预算数值由宿主显式给出，Engine 没有产品默认值。
//! 控制与导出函数与本地诊断一样须在原生后台工作队列调用，不能在 UI 主线程等待。
use std::sync::Mutex;
use std::time::Duration;

use crate::observability;
use crate::BindingObservabilitySignalResult;
use uc_engine::observability as engine;

macro_rules! mirror_enum {
    ($binding:ident, $core:ident, [$($variant:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
        pub enum $binding { $($variant),+ }
        impl From<engine::$core> for $binding { fn from(value: engine::$core) -> Self { match value { $(engine::$core::$variant => Self::$variant),+ } } }
        impl From<$binding> for engine::$core { fn from(value: $binding) -> Self { match value { $($binding::$variant => Self::$variant),+ } } }
    }
}

mirror_enum!(
    BindingNetworkProcessRole,
    NetworkProcessRole,
    [
        Daemon,
        MobileApp,
        KeyboardExtension,
        ShareExtension,
        BackgroundService
    ]
);
mirror_enum!(
    BindingNetworkLogState,
    NetworkLogState,
    [Starting, Ready, Unavailable, Closed]
);
mirror_enum!(
    BindingNetworkCaptureMode,
    NetworkCaptureMode,
    [Standard, Detailed]
);

/// 网络日志的角色与容量预算。没有默认值：数值由宿主显式给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkLogConfig {
    pub role: BindingNetworkProcessRole,
    /// 单条编码后的字节上限。
    pub record_bytes: u32,
    /// 写入队列的记录容量；队列满时新记录立即丢弃。
    pub queue_records: u32,
    /// 单个分段文件的字节上限。
    pub file_bytes: u64,
    /// 单次运行目录的字节上限；超出时淘汰最旧的分段。
    pub run_bytes: u64,
    /// 网络日志根目录的字节上限，只回收没有活动写入者的旧运行目录。
    pub root_bytes: u64,
    pub retention_seconds: u64,
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum BindingNetworkLogError {
    #[error("network logging was configured after the observability runtime was installed")]
    AlreadyInstalled,
    #[error("network log configuration is invalid")]
    InvalidConfig,
    #[error("observability is not installed")]
    NotInstalled,
    #[error("network logging is not configured for this process")]
    NotConfigured,
    #[error("capture duration is invalid")]
    InvalidDuration,
    #[error("capture identifier does not match the active window")]
    CaptureMismatch,
    #[error("network logging is shut down")]
    AlreadyShutdown,
    #[error("export deadline is invalid")]
    InvalidDeadline,
}

impl From<engine::NetworkLogError> for BindingNetworkLogError {
    fn from(value: engine::NetworkLogError) -> Self {
        match value {
            engine::NetworkLogError::NotConfigured => Self::NotConfigured,
            engine::NetworkLogError::InvalidDuration => Self::InvalidDuration,
            engine::NetworkLogError::CaptureMismatch => Self::CaptureMismatch,
            engine::NetworkLogError::AlreadyShutdown => Self::AlreadyShutdown,
            engine::NetworkLogError::InvalidDeadline => Self::InvalidDeadline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkLogCounters {
    pub accepted: u64,
    pub written: u64,
    pub policy_filtered: u64,
    pub rate_limited: u64,
    pub oversized: u64,
    pub queue_dropped: u64,
    pub closed_dropped: u64,
    pub write_failed: u64,
    pub rotated_out_files: u64,
    pub path_recovered: u64,
    pub unreviewed_callsites: u64,
}

impl From<engine::NetworkLogCounters> for BindingNetworkLogCounters {
    fn from(value: engine::NetworkLogCounters) -> Self {
        Self {
            accepted: value.accepted,
            written: value.written,
            policy_filtered: value.policy_filtered,
            rate_limited: value.rate_limited,
            oversized: value.oversized,
            queue_dropped: value.queue_dropped,
            closed_dropped: value.closed_dropped,
            write_failed: value.write_failed,
            rotated_out_files: value.rotated_out_files,
            path_recovered: value.path_recovered,
            unreviewed_callsites: value.unreviewed_callsites,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkLibraryStatus {
    pub library: String,
    pub version_dir: Option<String>,
    pub approved_rules: u32,
    pub observed_records: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkLogStatus {
    pub schema_version: u32,
    pub state: BindingNetworkLogState,
    pub run_id: String,
    pub role: BindingNetworkProcessRole,
    pub mode: BindingNetworkCaptureMode,
    pub detailed_remaining_ms: Option<u64>,
    pub counters: BindingNetworkLogCounters,
    pub libraries: Vec<BindingNetworkLibraryStatus>,
    pub sampled_at_utc: String,
}

impl From<engine::NetworkLogStatus> for BindingNetworkLogStatus {
    fn from(value: engine::NetworkLogStatus) -> Self {
        Self {
            schema_version: value.schema_version,
            state: value.state.into(),
            run_id: value.run_id,
            role: value.role.into(),
            mode: value.mode.into(),
            detailed_remaining_ms: value.detailed_remaining_ms,
            counters: value.counters.into(),
            libraries: value
                .libraries
                .into_iter()
                .map(|library| BindingNetworkLibraryStatus {
                    library: library.library.to_owned(),
                    version_dir: library.version_dir.map(str::to_owned),
                    approved_rules: u32::try_from(library.approved_rules).unwrap_or(u32::MAX),
                    observed_records: library.observed_records,
                })
                .collect(),
            sampled_at_utc: value.sampled_at_utc,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkCaptureStatus {
    pub capture_id: String,
    pub started_at_utc: String,
    pub duration_ms: u64,
    pub already_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkLogFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingNetworkLogExportReport {
    pub flush: BindingObservabilitySignalResult,
    pub status: BindingNetworkLogStatus,
    pub requested_at_utc: String,
    pub completed_at_utc: String,
    pub other_processes_flushed: bool,
    /// 本次运行目录的路径；旧运行由宿主按目录发现。
    pub run_directory: String,
    pub files: Vec<BindingNetworkLogFile>,
}

impl From<engine::NetworkLogExportReport> for BindingNetworkLogExportReport {
    fn from(value: engine::NetworkLogExportReport) -> Self {
        Self {
            flush: match value.flush {
                engine::ObservabilitySignalResult::Completed => {
                    BindingObservabilitySignalResult::Completed
                }
                engine::ObservabilitySignalResult::Failed => {
                    BindingObservabilitySignalResult::Failed
                }
                engine::ObservabilitySignalResult::TimedOut => {
                    BindingObservabilitySignalResult::TimedOut
                }
                engine::ObservabilitySignalResult::AlreadyShutdown => {
                    BindingObservabilitySignalResult::AlreadyShutdown
                }
            },
            status: value.status.into(),
            requested_at_utc: value.requested_at_utc,
            completed_at_utc: value.completed_at_utc,
            other_processes_flushed: value.other_processes_flushed,
            run_directory: value.run_directory.to_string_lossy().into_owned(),
            files: value
                .files
                .into_iter()
                .map(|file| BindingNetworkLogFile {
                    name: file.name,
                    bytes: file.bytes,
                    sha256: file.sha256,
                })
                .collect(),
        }
    }
}

/// 安装前登记的网络日志设置；安装时读取，不在安装后修改。
static PENDING: Mutex<Option<BindingNetworkLogConfig>> = Mutex::new(None);

pub(crate) fn pending_config(
    directories: &uc_engine::HostDirectories,
) -> Result<Option<engine::NetworkLogConfig>, crate::BindingError> {
    let Some(settings) = *PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    else {
        return Ok(None);
    };
    let budget = engine::NetworkLogBudget {
        record_bytes: usize::try_from(settings.record_bytes)
            // discarded-source[int-conversion]: the oversized value is rejected as invalid configuration
            .map_err(|_| crate::BindingError::ObservabilityConfigInvalid)?,
        queue_records: usize::try_from(settings.queue_records)
            // discarded-source[int-conversion]: the oversized value is rejected as invalid configuration
            .map_err(|_| crate::BindingError::ObservabilityConfigInvalid)?,
        file_bytes: settings.file_bytes,
        run_bytes: settings.run_bytes,
        root_bytes: settings.root_bytes,
        retention: Duration::from_secs(settings.retention_seconds),
    };
    engine::NetworkLogConfig::new(directories.logs(), settings.role.into(), budget)
        // discarded-source[input-validation]: the rejection reason is fully expressed by the target classification
        .map_err(|_| crate::BindingError::ObservabilityConfigInvalid)
        .map(|config| Some(config.with_known_alpns(engine::known_network_alpns())))
}

/// 在 `install_process_observability` 之前登记；安装之后再调用会被拒绝。
#[uniffi::export]
pub fn configure_network_log(
    config: BindingNetworkLogConfig,
) -> Result<(), BindingNetworkLogError> {
    if observability::process_handle().is_some() {
        return Err(BindingNetworkLogError::AlreadyInstalled);
    }
    let valid = config.record_bytes >= 512
        && config.queue_records > 0
        && config.file_bytes >= u64::from(config.record_bytes)
        && config.run_bytes >= config.file_bytes
        && config.root_bytes >= config.run_bytes
        && config.retention_seconds > 0;
    if !valid {
        return Err(BindingNetworkLogError::InvalidConfig);
    }
    *PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
    Ok(())
}

fn handle() -> Result<engine::ProcessObservabilityHandle, BindingNetworkLogError> {
    observability::process_handle().ok_or(BindingNetworkLogError::NotInstalled)
}

/// 开启当前进程的限时详细采集（1–900 秒）。重复开启不延长；窗口不跨重启。
#[uniffi::export]
pub fn start_network_capture(
    duration_ms: u64,
) -> Result<BindingNetworkCaptureStatus, BindingNetworkLogError> {
    let status = handle()?.start_network_capture(Duration::from_millis(duration_ms))?;
    Ok(BindingNetworkCaptureStatus {
        capture_id: status.capture_id,
        started_at_utc: status.started_at_utc,
        duration_ms: status.duration_ms,
        already_active: status.already_active,
    })
}

#[uniffi::export]
pub fn stop_network_capture(capture_id: String) -> Result<(), BindingNetworkLogError> {
    Ok(handle()?.stop_network_capture(&capture_id)?)
}

/// 未配置时返回 `NotConfigured`，不伪装成零事件。
#[uniffi::export]
pub fn query_network_log_status() -> Result<BindingNetworkLogStatus, BindingNetworkLogError> {
    Ok(handle()?.query_network_log_status()?.into())
}

/// 有界刷新本进程的网络日志并列出本次运行的文件；不代表扩展或其他进程已经刷新。
#[uniffi::export]
pub fn prepare_network_log_export(
    deadline_ms: u64,
) -> Result<BindingNetworkLogExportReport, BindingNetworkLogError> {
    Ok(handle()?
        .prepare_network_log_export(Duration::from_millis(deadline_ms))?
        .into())
}
