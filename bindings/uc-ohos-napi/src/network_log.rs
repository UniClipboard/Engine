//! HarmonyOS 第三方网络库独立日志入口；大计数用十进制字符串，避免 JS 整数精度丢失。
//!
//! 宿主在 `installProcessObservability` 之前调用 `configureNetworkLog` 登记角色与容量预算，
//! 未登记则整条链路不存在。预算数值由宿主显式给出，Engine 没有产品默认值。
use napi_derive::napi;
use std::sync::Mutex;
use std::time::Duration;
use uc_engine::observability as engine;
use uc_engine::HostDirectories;

use crate::observability;

#[napi]
pub enum OhNetworkProcessRole {
    Daemon,
    MobileApp,
    KeyboardExtension,
    ShareExtension,
    BackgroundService,
}

impl From<OhNetworkProcessRole> for engine::NetworkProcessRole {
    fn from(value: OhNetworkProcessRole) -> Self {
        match value {
            OhNetworkProcessRole::Daemon => Self::Daemon,
            OhNetworkProcessRole::MobileApp => Self::MobileApp,
            OhNetworkProcessRole::KeyboardExtension => Self::KeyboardExtension,
            OhNetworkProcessRole::ShareExtension => Self::ShareExtension,
            OhNetworkProcessRole::BackgroundService => Self::BackgroundService,
        }
    }
}

fn role_name(value: engine::NetworkProcessRole) -> &'static str {
    match value {
        engine::NetworkProcessRole::Daemon => "daemon",
        engine::NetworkProcessRole::MobileApp => "mobile_app",
        engine::NetworkProcessRole::KeyboardExtension => "keyboard_extension",
        engine::NetworkProcessRole::ShareExtension => "share_extension",
        engine::NetworkProcessRole::BackgroundService => "background_service",
    }
}

fn state_name(value: engine::NetworkLogState) -> &'static str {
    match value {
        engine::NetworkLogState::Starting => "starting",
        engine::NetworkLogState::Ready => "ready",
        engine::NetworkLogState::Unavailable => "unavailable",
        engine::NetworkLogState::Closed => "closed",
    }
}

fn mode_name(value: engine::NetworkCaptureMode) -> &'static str {
    match value {
        engine::NetworkCaptureMode::Standard => "standard",
        engine::NetworkCaptureMode::Detailed => "detailed",
    }
}

fn flush_name(value: engine::ObservabilitySignalResult) -> &'static str {
    match value {
        engine::ObservabilitySignalResult::Completed => "completed",
        engine::ObservabilitySignalResult::Failed => "failed",
        engine::ObservabilitySignalResult::TimedOut => "timed_out",
        engine::ObservabilitySignalResult::AlreadyShutdown => "already_shutdown",
    }
}

/// 字节容量以十进制字符串传入，避免 JS number 精度丢失。
#[napi(object)]
pub struct OhNetworkLogConfig {
    pub role: OhNetworkProcessRole,
    pub record_bytes: u32,
    pub queue_records: u32,
    pub file_bytes: String,
    pub run_bytes: String,
    pub root_bytes: String,
    pub retention_seconds: u32,
}

#[napi(object)]
pub struct OhNetworkLogCounters {
    pub accepted: String,
    pub written: String,
    pub policy_filtered: String,
    pub rate_limited: String,
    pub oversized: String,
    pub queue_dropped: String,
    pub closed_dropped: String,
    pub write_failed: String,
    pub rotated_out_files: String,
    pub path_recovered: String,
    pub unreviewed_callsites: String,
}

#[napi(object)]
pub struct OhNetworkLibraryStatus {
    pub library: String,
    pub version_dir: Option<String>,
    pub approved_rules: u32,
    pub observed_records: String,
}

#[napi(object)]
pub struct OhNetworkLogStatus {
    pub schema_version: u32,
    pub state: String,
    pub run_id: String,
    pub role: String,
    pub mode: String,
    pub detailed_remaining_ms: Option<String>,
    pub counters: OhNetworkLogCounters,
    pub libraries: Vec<OhNetworkLibraryStatus>,
    pub sampled_at_utc: String,
}

#[napi(object)]
pub struct OhNetworkCaptureStatus {
    pub capture_id: String,
    pub started_at_utc: String,
    pub duration_ms: String,
    pub already_active: bool,
}

#[napi(object)]
pub struct OhNetworkLogFile {
    pub name: String,
    pub bytes: String,
    pub sha256: String,
}

#[napi(object)]
pub struct OhNetworkLogExportReport {
    pub flush: String,
    pub status: OhNetworkLogStatus,
    pub requested_at_utc: String,
    pub completed_at_utc: String,
    pub other_processes_flushed: bool,
    pub run_directory: String,
    pub files: Vec<OhNetworkLogFile>,
}

fn status(value: engine::NetworkLogStatus) -> OhNetworkLogStatus {
    let c = value.counters;
    OhNetworkLogStatus {
        schema_version: value.schema_version,
        state: state_name(value.state).to_owned(),
        run_id: value.run_id,
        role: role_name(value.role).to_owned(),
        mode: mode_name(value.mode).to_owned(),
        detailed_remaining_ms: value.detailed_remaining_ms.map(|ms| ms.to_string()),
        counters: OhNetworkLogCounters {
            accepted: c.accepted.to_string(),
            written: c.written.to_string(),
            policy_filtered: c.policy_filtered.to_string(),
            rate_limited: c.rate_limited.to_string(),
            oversized: c.oversized.to_string(),
            queue_dropped: c.queue_dropped.to_string(),
            closed_dropped: c.closed_dropped.to_string(),
            write_failed: c.write_failed.to_string(),
            rotated_out_files: c.rotated_out_files.to_string(),
            path_recovered: c.path_recovered.to_string(),
            unreviewed_callsites: c.unreviewed_callsites.to_string(),
        },
        libraries: value
            .libraries
            .into_iter()
            .map(|library| OhNetworkLibraryStatus {
                library: library.library.to_owned(),
                version_dir: library.version_dir.map(str::to_owned),
                approved_rules: u32::try_from(library.approved_rules).unwrap_or(u32::MAX),
                observed_records: library.observed_records.to_string(),
            })
            .collect(),
        sampled_at_utc: value.sampled_at_utc,
    }
}

struct Pending {
    role: engine::NetworkProcessRole,
    budget: engine::NetworkLogBudget,
}

/// 安装前登记的网络日志设置；安装时读取，不在安装后修改。
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

pub(crate) fn pending_config(
    directories: &HostDirectories,
) -> napi::Result<Option<engine::NetworkLogConfig>> {
    let guard = PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(pending) = guard.as_ref() else {
        return Ok(None);
    };
    engine::NetworkLogConfig::new(directories.logs(), pending.role, pending.budget)
        // discarded-source[input-validation]: the rejection reason is fully expressed by the target classification
        .map_err(|_| invalid("network log configuration is invalid"))
        .map(|config| Some(config.with_known_alpns(engine::known_network_alpns())))
}

fn invalid(message: &str) -> napi::Error {
    napi::Error::new(napi::Status::InvalidArg, message)
}

fn bytes(text: &str) -> napi::Result<u64> {
    text.parse::<u64>()
        // discarded-source[input-validation]: only the invalid classification is reported
        .map_err(|_| invalid("network log byte value is invalid"))
}

#[napi]
pub fn configure_network_log(config: OhNetworkLogConfig) -> napi::Result<()> {
    if observability::process_handle().is_some() {
        return Err(napi::Error::new(
            napi::Status::GenericFailure,
            "network logging was configured after the observability runtime was installed",
        ));
    }
    let file_bytes = bytes(&config.file_bytes)?;
    let run_bytes = bytes(&config.run_bytes)?;
    let root_bytes = bytes(&config.root_bytes)?;
    let valid = config.record_bytes >= 512
        && config.queue_records > 0
        && file_bytes >= u64::from(config.record_bytes)
        && run_bytes >= file_bytes
        && root_bytes >= run_bytes
        && config.retention_seconds > 0;
    if !valid {
        return Err(invalid("network log configuration is invalid"));
    }
    let budget = engine::NetworkLogBudget {
        record_bytes: config.record_bytes as usize,
        queue_records: config.queue_records as usize,
        file_bytes,
        run_bytes,
        root_bytes,
        retention: Duration::from_secs(u64::from(config.retention_seconds)),
    };
    *PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Pending {
        role: config.role.into(),
        budget,
    });
    Ok(())
}

fn handle() -> napi::Result<engine::ProcessObservabilityHandle> {
    observability::process_handle().ok_or_else(|| {
        napi::Error::new(
            napi::Status::GenericFailure,
            "observability is not installed",
        )
    })
}

fn failure(error: engine::NetworkLogError) -> napi::Error {
    let status = match error {
        engine::NetworkLogError::InvalidDuration
        | engine::NetworkLogError::CaptureMismatch
        | engine::NetworkLogError::InvalidDeadline => napi::Status::InvalidArg,
        engine::NetworkLogError::NotConfigured | engine::NetworkLogError::AlreadyShutdown => {
            napi::Status::GenericFailure
        }
    };
    napi::Error::new(status, format!("{error}"))
}

#[napi]
pub fn start_network_capture(duration_ms: u32) -> napi::Result<OhNetworkCaptureStatus> {
    let value = handle()?
        .start_network_capture(Duration::from_millis(u64::from(duration_ms)))
        .map_err(failure)?;
    Ok(OhNetworkCaptureStatus {
        capture_id: value.capture_id,
        started_at_utc: value.started_at_utc,
        duration_ms: value.duration_ms.to_string(),
        already_active: value.already_active,
    })
}

#[napi]
pub fn stop_network_capture(capture_id: String) -> napi::Result<()> {
    handle()?.stop_network_capture(&capture_id).map_err(failure)
}

/// 未配置时报错，不伪装成零事件。
#[napi]
pub fn query_network_log_status() -> napi::Result<OhNetworkLogStatus> {
    handle()?
        .query_network_log_status()
        .map(status)
        .map_err(failure)
}

/// 有界刷新本进程的网络日志并列出本次运行的文件；不代表扩展或其他进程已经刷新。
#[napi]
pub async fn prepare_network_log_export(
    deadline_ms: u32,
) -> napi::Result<OhNetworkLogExportReport> {
    let handle = handle()?;
    tokio::task::spawn_blocking(move || {
        handle
            .prepare_network_log_export(Duration::from_millis(u64::from(deadline_ms)))
            .map_err(failure)
            .map(|report| OhNetworkLogExportReport {
                flush: flush_name(report.flush).to_owned(),
                status: status(report.status),
                requested_at_utc: report.requested_at_utc,
                completed_at_utc: report.completed_at_utc,
                other_processes_flushed: report.other_processes_flushed,
                run_directory: report.run_directory.to_string_lossy().into_owned(),
                files: report
                    .files
                    .into_iter()
                    .map(|file| OhNetworkLogFile {
                        name: file.name,
                        bytes: file.bytes.to_string(),
                        sha256: file.sha256,
                    })
                    .collect(),
            })
    })
    .await
    // discarded-source[no-information]: the error value carries no usable diagnostic information
    .map_err(|_| {
        napi::Error::new(
            napi::Status::GenericFailure,
            "network log export worker failed",
        )
    })?
}
