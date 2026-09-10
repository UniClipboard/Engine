//! UniFFI bindings for the public `uc-engine` interface.

#[cfg(target_os = "android")]
mod android;
mod observability;
mod runtime;

pub use runtime::{
    ActiveClipboard, Device, EntryNotResendableReason, InvitationAvailability, InvitationIssued,
    JoinSpaceRejectionReason, JoinSpaceStatus, JoinedSpace, LocalDevice, MobileEngine,
    PeerConnectionRefresh, RelaySaveResult, ResendEntryOutcome, SendReport, SessionRecovery,
    SpaceCreated, SpaceInvitation, SpaceState, WorkspaceConvergence,
    WorkspaceConvergenceFailureCategory, WorkspaceConvergencePhase,
};

uniffi::setup_scaffolding!();

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingErrorCategory {
    InvalidInput,
    InvalidState,
    Unauthorized,
    NotFound,
    Conflict,
    Unavailable,
    DeadlineExceeded,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BindingError {
    #[error("engine error {code} ({category:?})")]
    Engine {
        code: u32,
        category: BindingErrorCategory,
        retryable: bool,
    },
    #[error("host capability unavailable")]
    HostUnavailable,
    #[error("host capability permission denied")]
    HostPermissionDenied,
    #[error("host file handle invalid")]
    HostInvalidHandle,
    #[error("host input/output failed")]
    HostIo,
    #[error("binding runtime unavailable")]
    RuntimeUnavailable,
    #[error("binding engine already stopped")]
    AlreadyStopped,
    #[error("observability configuration is invalid")]
    ObservabilityConfigInvalid,
    #[error("observability runtime is already installed with a different configuration")]
    ObservabilityConfigConflict,
    #[error("observability runtime is unavailable")]
    ObservabilityRuntimeUnavailable,
    #[error("observability runtime is not installed")]
    ObservabilityNotInstalled,
    #[error("engine returned an unexpected result")]
    UnexpectedResult,
}

impl From<uc_engine::EngineError> for BindingError {
    fn from(error: uc_engine::EngineError) -> Self {
        Self::Engine {
            code: error.code(),
            category: map_error_category(error.category()),
            retryable: error.is_retryable(),
        }
    }
}

impl From<uc_engine::EngineError> for BindingFailure {
    fn from(error: uc_engine::EngineError) -> Self {
        Self {
            code: error.code(),
            category: map_error_category(error.category()),
            retryable: error.is_retryable(),
        }
    }
}

fn map_error_category(category: uc_engine::EngineErrorCategory) -> BindingErrorCategory {
    match category {
        uc_engine::EngineErrorCategory::InvalidInput => BindingErrorCategory::InvalidInput,
        uc_engine::EngineErrorCategory::InvalidState => BindingErrorCategory::InvalidState,
        uc_engine::EngineErrorCategory::Unauthorized => BindingErrorCategory::Unauthorized,
        uc_engine::EngineErrorCategory::NotFound => BindingErrorCategory::NotFound,
        uc_engine::EngineErrorCategory::Conflict => BindingErrorCategory::Conflict,
        uc_engine::EngineErrorCategory::Unavailable => BindingErrorCategory::Unavailable,
        uc_engine::EngineErrorCategory::DeadlineExceeded => BindingErrorCategory::DeadlineExceeded,
        uc_engine::EngineErrorCategory::Internal => BindingErrorCategory::Internal,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum HostBindingError {
    #[error("host capability unavailable")]
    Unavailable,
    #[error("host capability permission denied")]
    PermissionDenied,
    #[error("host file handle invalid")]
    InvalidHandle,
    #[error("host input/output failed")]
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BindingAnalyticsHostError {
    #[error("analytics context unavailable")]
    ContextUnavailable,
    #[error("analytics delivery failed")]
    DeliveryFailed,
    #[error("analytics identity persistence failed")]
    PersistenceFailed,
    #[error("analytics identity invalid")]
    InvalidIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingAnalyticsEvent {
    pub name: String,
    pub properties_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingAnalyticsOs {
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingAnalyticsDeviceType {
    Mobile,
    Desktop,
}

/// Platform attributes that the host supplies once for all analytics events.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingAnalyticsContext {
    pub os: BindingAnalyticsOs,
    pub os_version: String,
    pub device_type: BindingAnalyticsDeviceType,
    pub arch: String,
    pub app_channel: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingAnalyticsIdentityChange {
    pub previous_distinct_id: String,
    pub new_distinct_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingAnalyticsIdentify {
    pub old_distinct_id: String,
    pub new_distinct_id: String,
    pub set_json: String,
    pub set_once_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingAnalyticsGroupIdentify {
    pub group_type: String,
    pub group_key: String,
    pub set_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingConfig {
    pub app_version: String,
    pub profile_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingDeploymentEnvironment {
    Development,
    Test,
    Staging,
    Production,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingCollectorConfig {
    pub trace_endpoint: String,
    pub log_endpoint: String,
    pub auth_header_name: Option<String>,
    pub auth_header_value: Option<String>,
}

impl std::fmt::Debug for BindingCollectorConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BindingCollectorConfig(REDACTED)")
    }
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingObservabilityConfig {
    pub service_version: String,
    pub environment: BindingDeploymentEnvironment,
    pub app_channel: String,
    pub remote_diagnostics_enabled: bool,
    pub collector: Option<BindingCollectorConfig>,
}

impl std::fmt::Debug for BindingObservabilityConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BindingObservabilityConfig(REDACTED)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingObservabilitySetupStatus {
    Disabled,
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingObservabilitySetup {
    pub reused: bool,
    pub remote: BindingObservabilitySetupStatus,
    pub local_file: BindingObservabilitySetupStatus,
    pub dropped_local_records: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingObservabilityHealth {
    pub remote: BindingObservabilitySetupStatus,
    pub local_file: BindingObservabilitySetupStatus,
    pub dropped_local_records: u64,
    pub dropped_remote_spans: u64,
    pub dropped_remote_logs: u64,
    pub failed_remote_span_batches: u64,
    pub failed_remote_log_batches: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingObservabilitySignalResult {
    Completed,
    Failed,
    TimedOut,
    AlreadyShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingObservabilityFlushSummary {
    pub traces: BindingObservabilitySignalResult,
    pub logs: BindingObservabilitySignalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingObservabilityShutdownSummary {
    pub traces: BindingObservabilitySignalResult,
    pub logs: BindingObservabilitySignalResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingEngineState {
    Running,
    Quiescing,
    Quiesced,
    Suspended,
    ShuttingDown,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingLifecycleAction {
    Suspend,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingFailure {
    pub code: u32,
    pub category: BindingErrorCategory,
    pub retryable: bool,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingFileMetadata {
    pub display_name: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}

#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum BindingClipboardRepresentation {
    Inline {
        format: String,
        mime_type: Option<String>,
        bytes: Vec<u8>,
    },
    File {
        format: String,
        handle: String,
        display_name: String,
        mime_type: Option<String>,
        size_bytes: u64,
    },
}

impl std::fmt::Debug for BindingClipboardRepresentation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("BindingClipboardRepresentation");
        match self {
            Self::Inline {
                format,
                mime_type,
                bytes,
            } => debug
                .field("kind", &"inline")
                .field("format", format)
                .field("mime_type", mime_type)
                .field("byte_len", &bytes.len()),
            Self::File {
                format,
                mime_type,
                size_bytes,
                ..
            } => debug
                .field("kind", &"file")
                .field("format", format)
                .field("handle", &"[REDACTED]")
                .field("display_name", &"[REDACTED]")
                .field("mime_type", mime_type)
                .field("size_bytes", size_bytes),
        };
        debug.finish()
    }
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct BindingClipboardSnapshot {
    pub observed_at_ms: i64,
    pub representations: Vec<BindingClipboardRepresentation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingClipboardRestoreMode {
    Standard,
    PlainText,
    FilePaths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingClipboardRestoreOutcome {
    Restored,
    PayloadUnavailable,
    NotApplicable,
}

impl std::fmt::Debug for BindingClipboardSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BindingClipboardSnapshot")
            .field("observed_at_ms", &self.observed_at_ms)
            .field("representation_count", &self.representations.len())
            .finish()
    }
}

impl std::fmt::Debug for BindingFileMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BindingFileMetadata")
            .field("display_name", &"[REDACTED]")
            .field("size_bytes", &self.size_bytes)
            .field("mime_type", &self.mime_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingOperationTerminal {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingRefreshReason {
    ConsumerLagged,
    StateInvalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingClipboardOrigin {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingTransferDirection {
    Sending,
    Receiving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BindingRePairingScope {
    AllDevices,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum BindingEvent {
    StateChanged {
        state: BindingEngineState,
    },
    OperationFinished {
        operation_id: String,
        terminal: BindingOperationTerminal,
        failure: Option<BindingFailure>,
    },
    LifecycleFailed {
        action: BindingLifecycleAction,
        failure: BindingFailure,
    },
    RefreshRequired {
        reason: BindingRefreshReason,
    },
    Fatal {
        failure: BindingFailure,
    },
    IncomingEntry {
        entry_id: String,
        attempt_id: Option<String>,
        preview: String,
        origin: BindingClipboardOrigin,
    },
    IncomingPending {
        entry_id: String,
        attempt_id: Option<String>,
        from_device: String,
        total_bytes: Option<u64>,
        filenames: Vec<String>,
    },
    ReceiveAttemptStateChanged {
        entry_id: String,
        attempt_id: String,
        state: String,
    },
    DeliveryStatusChanged {
        entry_id: String,
        target_device_id: String,
    },
    PeerPresenceChanged {
        device_id: String,
        state: String,
        at_ms: i64,
    },
    DeviceTrustChanged {
        revision: u64,
    },
    TransferProgress {
        transfer_id: String,
        entry_id: Option<String>,
        attempt_id: Option<String>,
        peer_id: String,
        direction: BindingTransferDirection,
        completed_bytes: u64,
        total_bytes: Option<u64>,
    },
    TransferStatusChanged {
        transfer_id: String,
        entry_id: Option<String>,
        attempt_id: Option<String>,
        status: String,
        reason: Option<String>,
    },
    ActiveClipboardChanged {
        snapshot_hash: String,
        entry_id: String,
        activated_at_ms: i64,
        activated_by: String,
    },
    NetworkRecoveryChanged {
        phase: String,
        retryable: bool,
        next_retry_in_ms: Option<u64>,
    },
    RePairingRequired {
        scope: BindingRePairingScope,
    },
    Changed {
        kind: String,
    },
}

#[uniffi::export(with_foreign)]
pub trait BindingHost: Send + Sync {
    fn private_data_directory(&self) -> Result<String, HostBindingError>;
    fn cache_directory(&self) -> Result<String, HostBindingError>;
    fn temporary_directory(&self) -> Result<String, HostBindingError>;
    fn secure_storage_get(&self, key: String) -> Result<Option<Vec<u8>>, HostBindingError>;
    fn secure_storage_set(&self, key: String, value: Vec<u8>) -> Result<(), HostBindingError>;
    fn secure_storage_delete(&self, key: String) -> Result<(), HostBindingError>;
    fn file_metadata(&self, handle: String) -> Result<BindingFileMetadata, HostBindingError>;
    fn file_read_chunk(
        &self,
        handle: String,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostBindingError>;
    fn file_write_chunk(
        &self,
        handle: String,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), HostBindingError>;
    fn file_finish_write(&self, handle: String) -> Result<(), HostBindingError>;
    fn clipboard_read(&self) -> Result<BindingClipboardSnapshot, HostBindingError>;
    fn clipboard_write(&self, snapshot: BindingClipboardSnapshot) -> Result<(), HostBindingError>;
}

#[uniffi::export(with_foreign)]
pub trait BindingAnalyticsHost: Send + Sync {
    fn capture(&self, event: BindingAnalyticsEvent) -> Result<(), BindingAnalyticsHostError>;
    fn identify(&self, payload: BindingAnalyticsIdentify) -> Result<(), BindingAnalyticsHostError>;
    fn group_identify(
        &self,
        payload: BindingAnalyticsGroupIdentify,
    ) -> Result<(), BindingAnalyticsHostError>;
    fn adopt_space_person(
        &self,
        space_person_id: String,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError>;
    fn release_space_person(
        &self,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError>;
    fn current_space_person_id(&self) -> Result<Option<String>, BindingAnalyticsHostError>;
    fn reset_telemetry_identity(
        &self,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError>;
}

#[uniffi::export]
pub fn core_version() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

#[uniffi::export]
pub fn install_process_observability(
    config: BindingObservabilityConfig,
    host: std::sync::Arc<dyn BindingHost>,
) -> Result<BindingObservabilitySetup, BindingError> {
    let directories = runtime::host_directories(&host)?;
    observability::install(config, &directories)
}

#[uniffi::export]
pub fn flush_process_observability(
    deadline_ms: u64,
) -> Result<BindingObservabilityFlushSummary, BindingError> {
    observability::force_flush(std::time::Duration::from_millis(deadline_ms))
}

#[uniffi::export]
pub fn query_process_observability_health() -> Result<BindingObservabilityHealth, BindingError> {
    observability::health()
}

#[uniffi::export]
pub fn shutdown_process_observability(
    deadline_ms: u64,
) -> Result<BindingObservabilityShutdownSummary, BindingError> {
    observability::shutdown(std::time::Duration::from_millis(deadline_ms))
}
