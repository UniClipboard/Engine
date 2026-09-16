//! HarmonyOS 本地诊断入口；大计数用十进制字符串，避免 JS 整数精度丢失。
use napi_derive::napi;
use std::time::Duration;
use uc_engine::observability as engine;

use crate::observability;

macro_rules! mirror_input {
    ($name:ident, $core:ident, [$($variant:ident),+ $(,)?]) => {
        #[napi]
        pub enum $name { $($variant),+ }
        impl From<$name> for engine::$core { fn from(value: $name) -> Self { match value { $($name::$variant => Self::$variant),+ } } }
    }
}
macro_rules! enum_name {
    ($name:ident, $core:ident, {$($variant:ident => $text:literal),+ $(,)?}) => {
        fn $name(value: engine::$core) -> String { match value { $(engine::$core::$variant => $text),+ }.to_owned() }
    }
}

mirror_input!(
    OhHostDiagnosticSource,
    HostDiagnosticSource,
    [
        Application,
        ShareExtension,
        KeyboardExtension,
        BackgroundService
    ]
);
mirror_input!(
    OhHostDiagnosticAction,
    HostDiagnosticAction,
    [RuntimeStart, RuntimeStop, OwnershipAcquire, SecurityPrepare]
);
mirror_input!(
    OhHostDiagnosticFailure,
    HostDiagnosticFailure,
    [Unavailable, PermissionDenied, Locked, Busy, Unknown]
);
mirror_input!(
    OhHostLifecycleState,
    HostLifecycleState,
    [Foreground, Background]
);
mirror_input!(
    OhHostNetworkKind,
    HostNetworkKind,
    [Wifi, Cellular, Ethernet, Other, Unknown]
);
mirror_input!(
    OhSourceCapability,
    SourceCapability,
    [Supported, Partial, Unsupported, Unknown]
);
#[napi]
pub enum OhHostDiagnosticOutcome {
    Completed,
    Failed,
    Interrupted,
}

enum_name!(mode_name, LocalCaptureMode, {Standard => "standard", Detailed => "detailed"});
enum_name!(end_name, CaptureEndReason, {Expired => "expired", Requested => "requested", SuspensionExpiryUnknown => "suspension_expiry_unknown", RuntimeShutdown => "runtime_shutdown"});
enum_name!(capability_name, SourceCapability, {Supported => "supported", Partial => "partial", Unsupported => "unsupported", Unknown => "unknown"});
enum_name!(collection_name, SourceCollection, {Enabled => "enabled", Disabled => "disabled", Unavailable => "unavailable", NotRegistered => "not_registered"});
enum_name!(setup_name, ObservabilitySetupStatus, {Ready => "ready", Disabled => "disabled", Unavailable => "unavailable"});
enum_name!(flush_name, ObservabilitySignalResult, {Completed => "completed", Failed => "failed", TimedOut => "timed_out", AlreadyShutdown => "already_shutdown"});
enum_name!(receipt_name, HostDiagnosticRecordStatus, {Accepted => "accepted", PolicyFiltered => "policy_filtered", CapacityExceeded => "capacity_exceeded",
    InvalidToken => "invalid_token", InvalidSource => "invalid_source", NotRegistered => "not_registered", Unavailable => "unavailable", AlreadyShutdown => "already_shutdown"});
enum_name!(source_name, LocalDiagnosticSource, {Runtime => "runtime", Connections => "connections", AddressStorage => "address_storage",
    DnsDiscovery => "dns_discovery", MdnsDiscovery => "mdns_discovery", PkarrDiscovery => "pkarr_discovery", ConnectionPaths => "connection_paths",
    RelayRecovery => "relay_recovery", MembershipUpdates => "membership_updates", Sessions => "sessions", HostApplication => "host_application",
    HostShareExtension => "host_share_extension", HostKeyboardExtension => "host_keyboard_extension", HostBackgroundService => "host_background_service"});

#[napi(object)]
pub struct OhLocalCaptureStatus {
    pub mode: String,
    pub capture_id: Option<String>,
    pub remaining_ms: u32,
    pub started_at_utc: Option<String>,
    pub end_reason: Option<String>,
    pub last_capture_id: Option<String>,
    pub revision: String,
}
impl From<engine::LocalCaptureStatus> for OhLocalCaptureStatus {
    fn from(value: engine::LocalCaptureStatus) -> Self {
        Self {
            mode: mode_name(value.mode),
            capture_id: value.capture_id,
            remaining_ms: u32::try_from(value.remaining_ms).unwrap_or(u32::MAX),
            started_at_utc: value.started_at_utc,
            end_reason: value.end_reason.map(end_name),
            last_capture_id: value.last_capture_id,
            revision: value.revision.to_string(),
        }
    }
}

#[napi(object)]
pub struct OhSourceCoverage {
    pub source: String,
    pub capability: String,
    pub collection: String,
    pub observed_count: String,
    pub policy_filtered_count: String,
}
impl From<engine::SourceCoverage> for OhSourceCoverage {
    fn from(value: engine::SourceCoverage) -> Self {
        Self {
            source: source_name(value.source),
            capability: capability_name(value.capability),
            collection: collection_name(value.collection),
            observed_count: value.observed_count.to_string(),
            policy_filtered_count: value.policy_filtered_count.to_string(),
        }
    }
}

#[napi(object)]
pub struct OhFileSourceCounts {
    pub source: String,
    pub accepted_count: String,
    pub written_count: String,
    pub queue_dropped_count: String,
    pub quota_dropped_count: String,
    pub write_failed_count: String,
    pub last_written_at_ms: Option<String>,
}
impl From<engine::FileSourceCounts> for OhFileSourceCounts {
    fn from(value: engine::FileSourceCounts) -> Self {
        Self {
            source: source_name(value.source),
            accepted_count: value.accepted_count.to_string(),
            written_count: value.written_count.to_string(),
            queue_dropped_count: value.queue_dropped_count.to_string(),
            quota_dropped_count: value.quota_dropped_count.to_string(),
            write_failed_count: value.write_failed_count.to_string(),
            last_written_at_ms: value
                .last_written_at_ms
                .map(|timestamp| timestamp.to_string()),
        }
    }
}

#[napi(object)]
pub struct OhLocalDiagnosticStatus {
    pub run_id: String,
    pub capture: OhLocalCaptureStatus,
    pub observed_records: String,
    pub policy_filtered_records: String,
    pub schema_rejected_records: String,
    pub correlation_limited_records: String,
    pub engine_version: String,
    pub source_commit: String,
    pub counter_scope: String,
    pub sources: Vec<OhSourceCoverage>,
    pub local_file: String,
    pub closed: bool,
}
impl From<engine::LocalDiagnosticStatus> for OhLocalDiagnosticStatus {
    fn from(value: engine::LocalDiagnosticStatus) -> Self {
        Self {
            run_id: value.run_id,
            capture: value.capture.into(),
            observed_records: value.observed_records.to_string(),
            policy_filtered_records: value.policy_filtered_records.to_string(),
            schema_rejected_records: value.schema_rejected_records.to_string(),
            correlation_limited_records: value.correlation_limited_records.to_string(),
            engine_version: value.engine_version,
            source_commit: value.source_commit,
            counter_scope: value.counter_scope.into(),
            sources: value.sources.into_iter().map(Into::into).collect(),
            local_file: setup_name(value.local_file),
            closed: value.closed,
        }
    }
}

#[napi(object)]
pub struct OhLocalDiagnosticExportReport {
    pub flush: String,
    pub status: OhLocalDiagnosticStatus,
    pub requested_at_utc: String,
    pub completed_at_utc: String,
    pub other_processes_flushed: bool,
    pub files: Vec<OhFileSourceCounts>,
}
impl From<engine::LocalDiagnosticExportReport> for OhLocalDiagnosticExportReport {
    fn from(value: engine::LocalDiagnosticExportReport) -> Self {
        Self {
            flush: flush_name(value.flush),
            status: value.status.into(),
            requested_at_utc: value.requested_at_utc,
            completed_at_utc: value.completed_at_utc,
            other_processes_flushed: value.other_processes_flushed,
            files: value.files.into_iter().map(Into::into).collect(),
        }
    }
}

#[napi(object)]
pub struct OhHostDiagnosticReceipt {
    pub status: String,
    pub token: Option<String>,
}

fn handle() -> napi::Result<engine::ProcessObservabilityHandle> {
    observability::process_handle()
        .ok_or_else(|| failure(engine::LocalDiagnosticError::NotInstalled))
}
fn failure(error: engine::LocalDiagnosticError) -> napi::Error {
    let status = if matches!(
        error,
        engine::LocalDiagnosticError::InvalidDuration
            | engine::LocalDiagnosticError::InvalidCaptureId
            | engine::LocalDiagnosticError::InvalidDeadline
            | engine::LocalDiagnosticError::InvalidHostSource
    ) {
        napi::Status::InvalidArg
    } else {
        napi::Status::GenericFailure
    };
    napi::Error::new(status, error.to_string())
}
fn record(
    source: OhHostDiagnosticSource,
    event: engine::HostDiagnosticEvent,
) -> napi::Result<OhHostDiagnosticReceipt> {
    let receipt = handle()?.record_host_diagnostic(source.into(), event);
    Ok(OhHostDiagnosticReceipt {
        status: receipt_name(receipt.status),
        token: receipt.token,
    })
}

#[napi]
pub fn start_local_diagnostic_capture(duration_ms: u32) -> napi::Result<OhLocalCaptureStatus> {
    handle()?
        .start_local_diagnostic_capture(engine::DetailedCaptureRequest {
            duration: Duration::from_millis(u64::from(duration_ms)),
        })
        .map(Into::into)
        .map_err(failure)
}
#[napi]
pub fn stop_local_diagnostic_capture(capture_id: String) -> napi::Result<String> {
    handle()?
        .stop_local_diagnostic_capture(&capture_id)
        .map(|result| {
            match result {
                engine::StopCaptureResult::Stopped => "stopped",
                engine::StopCaptureResult::AlreadyStopped => "already_stopped",
                engine::StopCaptureResult::DifferentCapture => "different_capture",
            }
            .to_owned()
        })
        .map_err(failure)
}
#[napi]
pub fn query_local_diagnostic_status() -> napi::Result<OhLocalDiagnosticStatus> {
    Ok(handle()?.query_local_diagnostic_status().into())
}
#[napi]
pub async fn prepare_local_diagnostic_export(
    deadline_ms: u32,
) -> napi::Result<OhLocalDiagnosticExportReport> {
    let handle = handle()?;
    tokio::task::spawn_blocking(move || {
        handle
            .prepare_local_diagnostic_export(Duration::from_millis(u64::from(deadline_ms)))
            .map(Into::into)
            .map_err(failure)
    })
    .await
    .map_err(|_| {
        napi::Error::new(
            napi::Status::GenericFailure,
            "local diagnostic worker failed",
        )
    })?
}
#[napi]
pub fn register_host_diagnostic_source(
    source: OhHostDiagnosticSource,
    capability: OhSourceCapability,
) -> napi::Result<()> {
    handle()?
        .register_host_diagnostic_source(source.into(), capability.into())
        .map_err(failure)
}
#[napi]
pub fn begin_host_diagnostic(
    source: OhHostDiagnosticSource,
    action: OhHostDiagnosticAction,
) -> napi::Result<OhHostDiagnosticReceipt> {
    record(
        source,
        engine::HostDiagnosticEvent::Begin {
            action: action.into(),
        },
    )
}
#[napi]
pub fn finish_host_diagnostic(
    source: OhHostDiagnosticSource,
    token: String,
    outcome: OhHostDiagnosticOutcome,
    reason: Option<OhHostDiagnosticFailure>,
) -> napi::Result<OhHostDiagnosticReceipt> {
    let outcome = match (outcome, reason) {
        (OhHostDiagnosticOutcome::Completed, None) => engine::HostDiagnosticOutcome::Completed,
        (OhHostDiagnosticOutcome::Interrupted, None) => engine::HostDiagnosticOutcome::Interrupted,
        (OhHostDiagnosticOutcome::Failed, reason) => engine::HostDiagnosticOutcome::Failed(
            reason
                .map(Into::into)
                .unwrap_or(engine::HostDiagnosticFailure::Unknown),
        ),
        _ => {
            return Err(napi::Error::new(
                napi::Status::InvalidArg,
                "unexpected diagnostic failure reason",
            ))
        }
    };
    record(
        source,
        engine::HostDiagnosticEvent::Finish { token, outcome },
    )
}
#[napi]
pub fn record_host_lifecycle(
    source: OhHostDiagnosticSource,
    state: OhHostLifecycleState,
) -> napi::Result<OhHostDiagnosticReceipt> {
    record(
        source,
        engine::HostDiagnosticEvent::Lifecycle {
            state: state.into(),
        },
    )
}
#[napi]
pub fn record_host_network_change(
    source: OhHostDiagnosticSource,
    kind: OhHostNetworkKind,
    available: bool,
) -> napi::Result<OhHostDiagnosticReceipt> {
    record(
        source,
        engine::HostDiagnosticEvent::NetworkChanged {
            kind: kind.into(),
            available,
        },
    )
}
#[napi]
pub fn record_host_ownership_released(
    source: OhHostDiagnosticSource,
) -> napi::Result<OhHostDiagnosticReceipt> {
    record(source, engine::HostDiagnosticEvent::OwnershipReleased)
}
