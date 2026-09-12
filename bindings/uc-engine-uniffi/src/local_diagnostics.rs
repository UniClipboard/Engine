//! 本地诊断的附加绑定；原安装配置、业务入口和错误布局保持兼容。
use crate::observability;
use crate::{BindingObservabilitySetupStatus, BindingObservabilitySignalResult};
use std::time::Duration;
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
    BindingLocalCaptureMode,
    LocalCaptureMode,
    [Standard, Detailed]
);
mirror_enum!(
    BindingCaptureEndReason,
    CaptureEndReason,
    [Expired, Requested, SuspensionExpiryUnknown, RuntimeShutdown]
);
mirror_enum!(
    BindingStopCaptureResult,
    StopCaptureResult,
    [Stopped, AlreadyStopped, DifferentCapture]
);
mirror_enum!(
    BindingSourceCapability,
    SourceCapability,
    [Supported, Partial, Unsupported, Unknown]
);
mirror_enum!(
    BindingSourceCollection,
    SourceCollection,
    [Enabled, Disabled, Unavailable, NotRegistered]
);
mirror_enum!(
    BindingLocalDiagnosticSource,
    LocalDiagnosticSource,
    [
        Runtime,
        Connections,
        AddressStorage,
        DnsDiscovery,
        MdnsDiscovery,
        PkarrDiscovery,
        ConnectionPaths,
        RelayRecovery,
        MembershipUpdates,
        Sessions,
        HostApplication,
        HostShareExtension,
        HostKeyboardExtension,
        HostBackgroundService
    ]
);
mirror_enum!(
    BindingHostDiagnosticSource,
    HostDiagnosticSource,
    [
        Application,
        ShareExtension,
        KeyboardExtension,
        BackgroundService
    ]
);
mirror_enum!(
    BindingHostDiagnosticAction,
    HostDiagnosticAction,
    [RuntimeStart, RuntimeStop, OwnershipAcquire, SecurityPrepare]
);
mirror_enum!(
    BindingHostDiagnosticFailure,
    HostDiagnosticFailure,
    [Unavailable, PermissionDenied, Locked, Busy, Unknown]
);
mirror_enum!(
    BindingHostLifecycleState,
    HostLifecycleState,
    [Foreground, Background]
);
mirror_enum!(
    BindingHostNetworkKind,
    HostNetworkKind,
    [Wifi, Cellular, Ethernet, Other, Unknown]
);
mirror_enum!(
    BindingHostDiagnosticRecordStatus,
    HostDiagnosticRecordStatus,
    [
        Accepted,
        PolicyFiltered,
        CapacityExceeded,
        InvalidToken,
        InvalidSource,
        NotRegistered,
        Unavailable,
        AlreadyShutdown
    ]
);

#[derive(Debug, Clone, uniffi::Enum)]
pub enum BindingHostDiagnosticOutcome {
    Completed,
    Failed {
        reason: BindingHostDiagnosticFailure,
    },
    Interrupted,
}
impl From<BindingHostDiagnosticOutcome> for engine::HostDiagnosticOutcome {
    fn from(value: BindingHostDiagnosticOutcome) -> Self {
        match value {
            BindingHostDiagnosticOutcome::Completed => Self::Completed,
            BindingHostDiagnosticOutcome::Failed { reason } => Self::Failed(reason.into()),
            BindingHostDiagnosticOutcome::Interrupted => Self::Interrupted,
        }
    }
}

#[derive(Clone, uniffi::Enum)]
pub enum BindingHostDiagnosticEvent {
    Begin {
        action: BindingHostDiagnosticAction,
    },
    Finish {
        token: String,
        outcome: BindingHostDiagnosticOutcome,
    },
    Lifecycle {
        state: BindingHostLifecycleState,
    },
    NetworkChanged {
        kind: BindingHostNetworkKind,
        available: bool,
    },
    OwnershipReleased,
}
impl From<BindingHostDiagnosticEvent> for engine::HostDiagnosticEvent {
    fn from(value: BindingHostDiagnosticEvent) -> Self {
        match value {
            BindingHostDiagnosticEvent::Begin { action } => Self::Begin {
                action: action.into(),
            },
            BindingHostDiagnosticEvent::Finish { token, outcome } => Self::Finish {
                token,
                outcome: outcome.into(),
            },
            BindingHostDiagnosticEvent::Lifecycle { state } => Self::Lifecycle {
                state: state.into(),
            },
            BindingHostDiagnosticEvent::NetworkChanged { kind, available } => {
                Self::NetworkChanged {
                    kind: kind.into(),
                    available,
                }
            }
            BindingHostDiagnosticEvent::OwnershipReleased => Self::OwnershipReleased,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum BindingLocalDiagnosticError {
    #[error("local diagnostics are not installed")]
    NotInstalled,
    #[error("local diagnostic files are unavailable")]
    LocalSinkUnavailable,
    #[error("local diagnostics are shut down")]
    AlreadyShutdown,
    #[error("capture duration is invalid")]
    InvalidDuration,
    #[error("capture identifier is invalid")]
    InvalidCaptureId,
    #[error("export deadline is invalid")]
    InvalidDeadline,
    #[error("host source is invalid for the platform")]
    InvalidHostSource,
}
impl From<engine::LocalDiagnosticError> for BindingLocalDiagnosticError {
    fn from(value: engine::LocalDiagnosticError) -> Self {
        match value {
            engine::LocalDiagnosticError::NotInstalled => Self::NotInstalled,
            engine::LocalDiagnosticError::LocalSinkUnavailable => Self::LocalSinkUnavailable,
            engine::LocalDiagnosticError::AlreadyShutdown => Self::AlreadyShutdown,
            engine::LocalDiagnosticError::InvalidDuration => Self::InvalidDuration,
            engine::LocalDiagnosticError::InvalidCaptureId => Self::InvalidCaptureId,
            engine::LocalDiagnosticError::InvalidDeadline => Self::InvalidDeadline,
            engine::LocalDiagnosticError::InvalidHostSource => Self::InvalidHostSource,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingLocalCaptureStatus {
    pub mode: BindingLocalCaptureMode,
    pub capture_id: Option<String>,
    pub remaining_ms: u64,
    pub started_at_utc: Option<String>,
    pub end_reason: Option<BindingCaptureEndReason>,
    pub last_capture_id: Option<String>,
    pub revision: u64,
}
impl From<engine::LocalCaptureStatus> for BindingLocalCaptureStatus {
    fn from(value: engine::LocalCaptureStatus) -> Self {
        Self {
            mode: value.mode.into(),
            capture_id: value.capture_id,
            remaining_ms: value.remaining_ms,
            started_at_utc: value.started_at_utc,
            end_reason: value.end_reason.map(Into::into),
            last_capture_id: value.last_capture_id,
            revision: value.revision,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingSourceCoverage {
    pub source: BindingLocalDiagnosticSource,
    pub capability: BindingSourceCapability,
    pub collection: BindingSourceCollection,
    pub observed_count: u64,
    pub policy_filtered_count: u64,
}
impl From<engine::SourceCoverage> for BindingSourceCoverage {
    fn from(value: engine::SourceCoverage) -> Self {
        Self {
            source: value.source.into(),
            capability: value.capability.into(),
            collection: value.collection.into(),
            observed_count: value.observed_count,
            policy_filtered_count: value.policy_filtered_count,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingFileSourceCounts {
    pub source: BindingLocalDiagnosticSource,
    pub accepted_count: u64,
    pub written_count: u64,
    pub queue_dropped_count: u64,
    pub quota_dropped_count: u64,
    pub write_failed_count: u64,
    pub last_written_at_ms: Option<u64>,
}
impl From<engine::FileSourceCounts> for BindingFileSourceCounts {
    fn from(value: engine::FileSourceCounts) -> Self {
        Self {
            source: value.source.into(),
            accepted_count: value.accepted_count,
            written_count: value.written_count,
            queue_dropped_count: value.queue_dropped_count,
            quota_dropped_count: value.quota_dropped_count,
            write_failed_count: value.write_failed_count,
            last_written_at_ms: value.last_written_at_ms,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingLocalDiagnosticStatus {
    pub run_id: String,
    pub capture: BindingLocalCaptureStatus,
    pub observed_records: u64,
    pub policy_filtered_records: u64,
    pub schema_rejected_records: u64,
    pub correlation_limited_records: u64,
    pub engine_version: String,
    pub source_commit: String,
    pub counter_scope: String,
    pub sources: Vec<BindingSourceCoverage>,
    pub local_file: BindingObservabilitySetupStatus,
    pub closed: bool,
}
impl From<engine::LocalDiagnosticStatus> for BindingLocalDiagnosticStatus {
    fn from(value: engine::LocalDiagnosticStatus) -> Self {
        Self {
            run_id: value.run_id,
            capture: value.capture.into(),
            observed_records: value.observed_records,
            policy_filtered_records: value.policy_filtered_records,
            schema_rejected_records: value.schema_rejected_records,
            correlation_limited_records: value.correlation_limited_records,
            engine_version: value.engine_version,
            source_commit: value.source_commit,
            counter_scope: value.counter_scope.into(),
            sources: value.sources.into_iter().map(Into::into).collect(),
            local_file: match value.local_file {
                engine::ObservabilitySetupStatus::Ready => BindingObservabilitySetupStatus::Ready,
                engine::ObservabilitySetupStatus::Disabled => {
                    BindingObservabilitySetupStatus::Disabled
                }
                engine::ObservabilitySetupStatus::Unavailable => {
                    BindingObservabilitySetupStatus::Unavailable
                }
            },
            closed: value.closed,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingLocalDiagnosticExportReport {
    pub flush: BindingObservabilitySignalResult,
    pub status: BindingLocalDiagnosticStatus,
    pub requested_at_utc: String,
    pub completed_at_utc: String,
    pub other_processes_flushed: bool,
    pub files: Vec<BindingFileSourceCounts>,
}
impl From<engine::LocalDiagnosticExportReport> for BindingLocalDiagnosticExportReport {
    fn from(value: engine::LocalDiagnosticExportReport) -> Self {
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
            files: value.files.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingHostDiagnosticReceipt {
    pub status: BindingHostDiagnosticRecordStatus,
    pub token: Option<String>,
}

fn handle() -> Result<engine::ProcessObservabilityHandle, BindingLocalDiagnosticError> {
    observability::process_handle().ok_or(BindingLocalDiagnosticError::NotInstalled)
}

#[uniffi::export]
pub fn start_local_diagnostic_capture(
    duration_ms: u64,
) -> Result<BindingLocalCaptureStatus, BindingLocalDiagnosticError> {
    Ok(handle()?
        .start_local_diagnostic_capture(engine::DetailedCaptureRequest {
            duration: Duration::from_millis(duration_ms),
        })?
        .into())
}
#[uniffi::export]
pub fn stop_local_diagnostic_capture(
    capture_id: String,
) -> Result<BindingStopCaptureResult, BindingLocalDiagnosticError> {
    Ok(handle()?.stop_local_diagnostic_capture(&capture_id)?.into())
}
#[uniffi::export]
pub fn query_local_diagnostic_status(
) -> Result<BindingLocalDiagnosticStatus, BindingLocalDiagnosticError> {
    Ok(handle()?.query_local_diagnostic_status().into())
}

/// 与原有 flush 一样，宿主须在原生后台工作队列调用，不能在 UI 主线程等待。
#[uniffi::export]
pub fn prepare_local_diagnostic_export(
    deadline_ms: u64,
) -> Result<BindingLocalDiagnosticExportReport, BindingLocalDiagnosticError> {
    Ok(handle()?
        .prepare_local_diagnostic_export(Duration::from_millis(deadline_ms))?
        .into())
}
#[uniffi::export]
pub fn register_host_diagnostic_source(
    source: BindingHostDiagnosticSource,
    capability: BindingSourceCapability,
) -> Result<(), BindingLocalDiagnosticError> {
    Ok(handle()?.register_host_diagnostic_source(source.into(), capability.into())?)
}
#[uniffi::export]
pub fn record_host_diagnostic(
    source: BindingHostDiagnosticSource,
    event: BindingHostDiagnosticEvent,
) -> Result<BindingHostDiagnosticReceipt, BindingLocalDiagnosticError> {
    let receipt = handle()?.record_host_diagnostic(source.into(), event.into());
    Ok(BindingHostDiagnosticReceipt {
        status: receipt.status.into(),
        token: receipt.token,
    })
}
