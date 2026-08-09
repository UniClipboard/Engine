use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::warn;
use uc_engine::observability::{
    AdoptOutcome, AnalyticsEventContext, AnalyticsIdentityError, AnalyticsIdentityPort,
    AnalyticsPort, DeviceType, Event, GroupIdentifyPayload, IdentifyPayload, Os, ReleaseOutcome,
};
use uc_engine::{
    ClipboardRestoreMode, ClipboardRestoreOutcome, ContinueMemberRevocationInput, CreateSpaceInput,
    Engine, EngineConfig, ExportEntryInput, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot,
    HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage,
    JoinSpaceInput, NetworkSettingsPatch, ObserveClipboardChangeInput, Operation, OperationResult,
    QueryMemberRevocationInput, QuerySharedDeviceRefreshInput, RecoverSessionInput,
    RelayCredentialEdit, RemoveMemberInput, ResendEntryInput, RestoreClipboardInput,
    SaveRelayInput, SaveRelayOutcome, SecretString, SendFilesInput, SendImageInput, SendTextInput,
    SettingsPatch,
};
use zeroize::Zeroizing;

use crate::{
    BindingAnalyticsContext, BindingAnalyticsDeviceType, BindingAnalyticsHost, BindingAnalyticsOs,
    BindingClipboardOrigin, BindingClipboardRepresentation, BindingClipboardRestoreMode,
    BindingClipboardRestoreOutcome, BindingClipboardSnapshot, BindingConfig, BindingEngineState,
    BindingError, BindingErrorCategory, BindingEvent, BindingFailure, BindingFileMetadata,
    BindingHost, BindingLifecycleAction, BindingOperationTerminal, BindingRefreshReason,
    BindingTransferDirection, HostBindingError,
};

const LIFECYCLE_TRANSITION_DEADLINE: Duration = Duration::from_secs(10);

fn log_mobile_query_failure(operation: &'static str, error: &BindingError) {
    match error {
        BindingError::Engine {
            code,
            category,
            retryable,
        } => warn!(
            operation,
            error_kind = "engine",
            error_code = *code,
            error_category = ?category,
            retryable = *retryable,
            "mobile query failed"
        ),
        BindingError::HostUnavailable => {
            warn!(
                operation,
                error_kind = "host_unavailable",
                "mobile query failed"
            )
        }
        BindingError::HostPermissionDenied => {
            warn!(
                operation,
                error_kind = "host_permission_denied",
                "mobile query failed"
            )
        }
        BindingError::HostInvalidHandle => {
            warn!(
                operation,
                error_kind = "host_invalid_handle",
                "mobile query failed"
            )
        }
        BindingError::HostIo => warn!(operation, error_kind = "host_io", "mobile query failed"),
        BindingError::RuntimeUnavailable => {
            warn!(
                operation,
                error_kind = "runtime_unavailable",
                "mobile query failed"
            )
        }
        BindingError::AlreadyStopped => {
            warn!(
                operation,
                error_kind = "already_stopped",
                "mobile query failed"
            )
        }
        BindingError::UnexpectedResult => {
            warn!(
                operation,
                error_kind = "unexpected_result",
                "mobile query failed"
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceCreated {
    pub space_id: String,
    pub self_device_id: String,
    pub identity_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SessionRecovery {
    pub unlocked: bool,
    pub resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ActiveClipboard {
    pub entry_id: String,
    pub activated_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalDevice {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct InvitationIssued {
    pub invitation_code: String,
    pub expires_at_ms: i64,
    pub availability: InvitationAvailability,
}

impl std::fmt::Debug for InvitationIssued {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvitationIssued")
            .field("invitation_code", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("availability", &self.availability)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum InvitationAvailability {
    CrossNetwork,
    SameLocalNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceJoined {
    pub sponsor_device_id: String,
    pub sponsor_identity_fingerprint: String,
    pub space_id: String,
    pub self_device_id: String,
    pub self_identity_fingerprint: String,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SendReport {
    pub entry_id: String,
    pub at_ms: i64,
    pub total_accepted: u64,
    pub total_duplicate: u64,
    pub total_offline: u64,
    pub total_errored: u64,
    pub total_pending: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeerConnectionRefresh {
    pub total: u64,
    pub online: u64,
    pub offline: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NetworkRecoveryStatus {
    pub phase: String,
    pub retryable: bool,
    pub next_retry_in_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceInvitation {
    pub invitation_code: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceState {
    pub has_completed: bool,
    pub space_id: Option<String>,
    pub current_invitation: Option<SpaceInvitation>,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Device {
    pub device_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MembershipConvergenceState {
    Complete,
    Converging,
    WaitingForUpgrade,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MembershipConvergence {
    pub state: MembershipConvergenceState,
    pub pending_count: u64,
    pub waiting_for_peer_count: u64,
    pub waiting_for_update_count: u64,
    pub version_incompatible_count: u64,
    pub blocked_count: u64,
    pub rejected_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SharedDeviceRefreshPhase {
    Started,
    Discovering,
    Connecting,
    RoundCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SharedDeviceRefreshDeviceState {
    Discovered,
    Connecting,
    Connected,
    AlreadyPresent,
    WaitingForPeer,
    WaitingForUpdate,
    VersionIncompatible,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedDeviceRefreshDevice {
    pub device_id: String,
    pub display_name: String,
    pub state: SharedDeviceRefreshDeviceState,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedDeviceRefreshStarted {
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedDeviceRefresh {
    pub request_id: String,
    pub phase: SharedDeviceRefreshPhase,
    pub devices: Vec<SharedDeviceRefreshDevice>,
    pub total_count: u64,
    pub discovered_count: u64,
    pub connecting_count: u64,
    pub connected_count: u64,
    pub already_present_count: u64,
    pub waiting_for_peer_count: u64,
    pub waiting_for_update_count: u64,
    pub version_incompatible_count: u64,
    pub rejected_count: u64,
    pub unavailable_source_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MemberRevocationOutcome {
    LocalOnly,
    Recovering,
    Applied,
    Complete,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MemberRevocationResult {
    pub revocation_id: Option<String>,
    pub outcome: MemberRevocationOutcome,
    pub pending_recipients: u64,
    pub removed_device_ids: Vec<String>,
    pub pending_recipient_device_ids: Vec<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LegacyMemberRemovalOutcome {
    AwaitingReadmission,
    Complete,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LegacyMemberRemovalResult {
    pub bootstrap_id: String,
    pub outcome: LegacyMemberRemovalOutcome,
    pub pending_readmission: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EntryNotResendableReason {
    RemoteOrigin,
    PayloadLost,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ResendEntryOutcome {
    Completed {
        accepted: u64,
        duplicate: u64,
        offline: u64,
        errored: u64,
        pending: u64,
    },
    EntryNotFound {
        entry_id: String,
    },
    EntryNotResendable {
        entry_id: String,
        reason: EntryNotResendableReason,
    },
    TargetNotTrusted {
        device_id: String,
    },
    NoEligibleTargets,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelaySaveResult {
    pub configured: bool,
}

enum WorkerCommand {
    RecoverSession {
        allow_secure_storage_unlock: bool,
        response: mpsc::Sender<Result<SessionRecovery, BindingError>>,
    },
    QueryLocalDevice {
        response: mpsc::Sender<Result<LocalDevice, BindingError>>,
    },
    RefreshPeerConnections {
        response: mpsc::Sender<Result<PeerConnectionRefresh, BindingError>>,
    },
    SaveCustomRelay {
        url: Zeroizing<String>,
        access_token: Zeroizing<String>,
        previous_url: Option<String>,
        response: mpsc::Sender<Result<RelaySaveResult, BindingError>>,
    },
    RecoverNetwork {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    QueryNetworkRecoveryStatus {
        response: mpsc::Sender<Result<NetworkRecoveryStatus, BindingError>>,
    },
    QuerySpaceState {
        response: mpsc::Sender<Result<SpaceState, BindingError>>,
    },
    ListDevices {
        response: mpsc::Sender<Result<Vec<Device>, BindingError>>,
    },
    QueryMembershipConvergence {
        response: mpsc::Sender<Result<MembershipConvergence, BindingError>>,
    },
    RefreshSharedDevices {
        response: mpsc::Sender<Result<SharedDeviceRefreshStarted, BindingError>>,
    },
    QuerySharedDeviceRefresh {
        request_id: String,
        response: mpsc::Sender<Result<Option<SharedDeviceRefresh>, BindingError>>,
    },
    RemoveMember {
        device_id: String,
        response: mpsc::Sender<Result<MemberRevocationResult, BindingError>>,
    },
    SecureRemoveLegacyMember {
        device_id: String,
        response: mpsc::Sender<Result<LegacyMemberRemovalResult, BindingError>>,
    },
    QueryMemberRevocation {
        revocation_id: String,
        response: mpsc::Sender<Result<Option<MemberRevocationResult>, BindingError>>,
    },
    QueryCurrentMemberRevocation {
        response: mpsc::Sender<Result<Option<MemberRevocationResult>, BindingError>>,
    },
    ContinueMemberRevocation {
        revocation_id: String,
        permanently_lost_device_ids: Vec<String>,
        response: mpsc::Sender<Result<MemberRevocationResult, BindingError>>,
    },
    ResendEntry {
        entry_id: String,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<ResendEntryOutcome, BindingError>>,
    },
    LeaveSpace {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    LifecycleState {
        response: mpsc::Sender<BindingEngineState>,
    },
    CreateSpace {
        device_name: Option<String>,
        passphrase: Zeroizing<String>,
        response: mpsc::Sender<Result<SpaceCreated, BindingError>>,
    },
    IssueInvitation {
        response: mpsc::Sender<Result<InvitationIssued, BindingError>>,
    },
    JoinSpace {
        invitation_code: Zeroizing<String>,
        device_name: Option<String>,
        passphrase: Zeroizing<String>,
        preserve_unreadable_history: bool,
        response: mpsc::Sender<Result<SpaceJoined, BindingError>>,
    },
    SendText {
        text: Zeroizing<String>,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<SendReport, BindingError>>,
    },
    SendImage {
        bytes: Zeroizing<Vec<u8>>,
        mime_type: String,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<SendReport, BindingError>>,
    },
    SendFiles {
        file_handles: Zeroizing<Vec<String>>,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<SendReport, BindingError>>,
    },
    CaptureCurrentClipboard {
        response: mpsc::Sender<Result<Option<String>, BindingError>>,
    },
    ObserveClipboardChange {
        dispatch: bool,
        response: mpsc::Sender<Result<Option<SendReport>, BindingError>>,
    },
    RestoreClipboard {
        entry_id: String,
        mode: BindingClipboardRestoreMode,
        response: mpsc::Sender<Result<BindingClipboardRestoreOutcome, BindingError>>,
    },
    QueryActiveClipboard {
        response: mpsc::Sender<Result<Option<ActiveClipboard>, BindingError>>,
    },
    ExportEntry {
        entry_id: String,
        destination_handle: Zeroizing<String>,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Suspend {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Resume {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Shutdown {
        deadline: Duration,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
}

#[derive(uniffi::Object)]
pub struct MobileEngine {
    commands: Mutex<Option<tokio::sync::mpsc::UnboundedSender<WorkerCommand>>>,
    lifecycle_commands: Mutex<Option<tokio::sync::mpsc::UnboundedSender<WorkerCommand>>>,
    events: Arc<EventQueue>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct EventQueueState {
    events: VecDeque<BindingEvent>,
    lagged: bool,
    closed: bool,
}

struct EventQueue {
    capacity: usize,
    state: Mutex<EventQueueState>,
    ready: Condvar,
}

struct BindingAnalyticsAdapter {
    host: Arc<dyn BindingAnalyticsHost>,
    context: AnalyticsEventContext,
}

impl BindingAnalyticsAdapter {
    fn new(host: Arc<dyn BindingAnalyticsHost>, context: AnalyticsEventContext) -> Self {
        Self { host, context }
    }

    fn map_identity_error(error: crate::BindingAnalyticsHostError) -> AnalyticsIdentityError {
        match error {
            crate::BindingAnalyticsHostError::ContextUnavailable => {
                AnalyticsIdentityError::ContextNotInitialised
            }
            crate::BindingAnalyticsHostError::DeliveryFailed
            | crate::BindingAnalyticsHostError::PersistenceFailed
            | crate::BindingAnalyticsHostError::InvalidIdentity => {
                AnalyticsIdentityError::PersistFailed(
                    std::io::Error::other("mobile analytics identity operation failed").into(),
                )
            }
        }
    }

    fn parse_adopt_outcome(
        change: crate::BindingAnalyticsIdentityChange,
        expected_new_id: uuid::Uuid,
    ) -> Result<AdoptOutcome, AnalyticsIdentityError> {
        let previous_distinct_id = change.previous_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        let new_distinct_id = change.new_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        if new_distinct_id != expected_new_id {
            return Err(Self::map_identity_error(
                crate::BindingAnalyticsHostError::InvalidIdentity,
            ));
        }
        Ok(AdoptOutcome {
            previous_distinct_id,
            new_distinct_id,
        })
    }

    fn parse_release_outcome(
        change: crate::BindingAnalyticsIdentityChange,
    ) -> Result<ReleaseOutcome, AnalyticsIdentityError> {
        let previous_distinct_id = change.previous_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        let new_distinct_id = change.new_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        Ok(ReleaseOutcome {
            previous_distinct_id,
            new_distinct_id,
        })
    }

    fn warn_callback(scope: &'static str, error: &crate::BindingAnalyticsHostError) {
        tracing::warn!(scope, error = %error, "mobile analytics callback failed");
    }
}

impl AnalyticsPort for BindingAnalyticsAdapter {
    fn capture(&self, event: Event) {
        let mut properties = event.properties();
        properties.extend(self.context.properties());
        let event = crate::BindingAnalyticsEvent {
            name: event.name().to_owned(),
            properties_json: serde_json::Value::Object(properties).to_string(),
        };
        if let Err(error) = self.host.capture(event) {
            Self::warn_callback("capture", &error);
        }
    }

    fn identify(&self, payload: IdentifyPayload) {
        let payload = crate::BindingAnalyticsIdentify {
            old_distinct_id: payload.old_distinct_id.to_string(),
            new_distinct_id: payload.new_distinct_id.to_string(),
            set_json: serde_json::Value::Object(payload.set).to_string(),
            set_once_json: serde_json::Value::Object(payload.set_once).to_string(),
        };
        if let Err(error) = self.host.identify(payload) {
            Self::warn_callback("identify", &error);
        }
    }

    fn group_identify(&self, payload: GroupIdentifyPayload) {
        let payload = crate::BindingAnalyticsGroupIdentify {
            group_type: payload.group_type,
            group_key: payload.group_key,
            set_json: serde_json::Value::Object(payload.set).to_string(),
        };
        if let Err(error) = self.host.group_identify(payload) {
            Self::warn_callback("group_identify", &error);
        }
    }
}

impl AnalyticsIdentityPort for BindingAnalyticsAdapter {
    fn adopt_space_person(
        &self,
        space_person_id: uuid::Uuid,
    ) -> Result<AdoptOutcome, AnalyticsIdentityError> {
        let change = self
            .host
            .adopt_space_person(space_person_id.to_string())
            .map_err(Self::map_identity_error)?;
        Self::parse_adopt_outcome(change, space_person_id)
    }

    fn release_space_person(&self) -> Result<ReleaseOutcome, AnalyticsIdentityError> {
        let change = self
            .host
            .release_space_person()
            .map_err(Self::map_identity_error)?;
        Self::parse_release_outcome(change)
    }

    fn current_space_person_id(&self) -> Option<uuid::Uuid> {
        match self.host.current_space_person_id() {
            Ok(Some(value)) => match value.parse() {
                Ok(value) => Some(value),
                Err(_) => {
                    Self::warn_callback(
                        "current_space_person_id",
                        &crate::BindingAnalyticsHostError::InvalidIdentity,
                    );
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                Self::warn_callback("current_space_person_id", &error);
                None
            }
        }
    }

    fn reset_telemetry_identity(&self) -> Result<ReleaseOutcome, AnalyticsIdentityError> {
        let change = self
            .host
            .reset_telemetry_identity()
            .map_err(Self::map_identity_error)?;
        Self::parse_release_outcome(change)
    }
}

impl EventQueue {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(EventQueueState {
                events: VecDeque::new(),
                lagged: false,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn push(&self, event: BindingEvent) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        if state.events.len() == self.capacity {
            state.events.pop_front();
            state.lagged = true;
        }
        state.events.push_back(event);
        self.ready.notify_one();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.ready.notify_all();
    }

    fn next(&self, timeout: Duration) -> Option<BindingEvent> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = lock(&self.state);
        loop {
            if state.lagged {
                state.lagged = false;
                return Some(BindingEvent::RefreshRequired {
                    reason: BindingRefreshReason::ConsumerLagged,
                });
            }
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            if state.closed || timeout.is_zero() {
                return None;
            }
            let remaining = deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(60));
            if remaining.is_zero() {
                return None;
            }
            state = match self.ready.wait_timeout(state, remaining) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

impl MobileEngine {
    fn start_inner(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
        analytics: Option<(Arc<dyn BindingAnalyticsHost>, BindingAnalyticsContext)>,
    ) -> Result<Arc<Self>, BindingError> {
        let capabilities = host_capabilities(Arc::clone(&host), analytics)?;
        let logs_dir = capabilities.directories().logs();
        #[cfg(target_vendor = "apple")]
        crate::apple::install_apple_tracing(logs_dir);
        #[cfg(target_os = "android")]
        crate::android::install_android_tracing(logs_dir);
        let config = EngineConfig::new(config.app_version).with_profile_id(config.profile_id);
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(256));
        let (started, start_result) = mpsc::channel();
        let worker_events = Arc::clone(&events);
        let worker = std::thread::Builder::new()
            .name("uc-engine-uniffi".to_owned())
            .spawn(move || {
                run_worker(
                    config,
                    capabilities,
                    requests,
                    lifecycle_requests,
                    worker_events,
                    started,
                )
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;

        match start_result.recv() {
            Ok(Ok(())) => Ok(Arc::new(Self {
                commands: Mutex::new(Some(commands)),
                lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
                events,
                worker: Mutex::new(Some(worker)),
            })),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(BindingError::RuntimeUnavailable)
            }
        }
    }
}

#[uniffi::export]
impl MobileEngine {
    #[uniffi::constructor]
    pub fn start(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
    ) -> Result<Arc<Self>, BindingError> {
        Self::start_inner(config, host, None)
    }

    #[uniffi::constructor]
    pub fn start_with_analytics(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
        analytics: Arc<dyn BindingAnalyticsHost>,
        context: BindingAnalyticsContext,
    ) -> Result<Arc<Self>, BindingError> {
        Self::start_inner(config, host, Some((analytics, context)))
    }

    pub fn recover_session(
        &self,
        allow_secure_storage_unlock: bool,
    ) -> Result<SessionRecovery, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RecoverSession {
                allow_secure_storage_unlock,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn query_local_device(&self) -> Result<LocalDevice, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::QueryLocalDevice { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn refresh_peer_connections(&self) -> Result<PeerConnectionRefresh, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RefreshPeerConnections { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn save_custom_relay(
        &self,
        url: String,
        access_token: String,
        previous_url: Option<String>,
    ) -> Result<RelaySaveResult, BindingError> {
        self.request(|response| WorkerCommand::SaveCustomRelay {
            url: Zeroizing::new(url),
            access_token: Zeroizing::new(access_token),
            previous_url,
            response,
        })
    }

    pub fn recover_network(&self) -> Result<(), BindingError> {
        self.request(|response| WorkerCommand::RecoverNetwork { response })
    }

    pub fn query_network_recovery_status(&self) -> Result<NetworkRecoveryStatus, BindingError> {
        self.request(|response| WorkerCommand::QueryNetworkRecoveryStatus { response })
    }

    pub fn query_space_state(&self) -> Result<SpaceState, BindingError> {
        self.request(|response| WorkerCommand::QuerySpaceState { response })
    }

    pub fn list_devices(&self) -> Result<Vec<Device>, BindingError> {
        self.request(|response| WorkerCommand::ListDevices { response })
    }

    pub fn query_membership_convergence(&self) -> Result<MembershipConvergence, BindingError> {
        self.request(|response| WorkerCommand::QueryMembershipConvergence { response })
    }

    pub fn refresh_shared_devices(&self) -> Result<SharedDeviceRefreshStarted, BindingError> {
        self.request(|response| WorkerCommand::RefreshSharedDevices { response })
    }

    pub fn query_shared_device_refresh(
        &self,
        request_id: String,
    ) -> Result<Option<SharedDeviceRefresh>, BindingError> {
        self.request(|response| WorkerCommand::QuerySharedDeviceRefresh {
            request_id,
            response,
        })
    }

    pub fn remove_member(&self, device_id: String) -> Result<MemberRevocationResult, BindingError> {
        self.request(|response| WorkerCommand::RemoveMember {
            device_id,
            response,
        })
    }

    pub fn secure_remove_legacy_member(
        &self,
        device_id: String,
    ) -> Result<LegacyMemberRemovalResult, BindingError> {
        self.request(|response| WorkerCommand::SecureRemoveLegacyMember {
            device_id,
            response,
        })
    }

    pub fn query_member_revocation(
        &self,
        revocation_id: String,
    ) -> Result<Option<MemberRevocationResult>, BindingError> {
        self.request(|response| WorkerCommand::QueryMemberRevocation {
            revocation_id,
            response,
        })
    }

    pub fn query_current_member_revocation(
        &self,
    ) -> Result<Option<MemberRevocationResult>, BindingError> {
        self.request(|response| WorkerCommand::QueryCurrentMemberRevocation { response })
    }

    pub fn continue_member_revocation(
        &self,
        revocation_id: String,
        permanently_lost_device_ids: Vec<String>,
    ) -> Result<MemberRevocationResult, BindingError> {
        self.request(|response| WorkerCommand::ContinueMemberRevocation {
            revocation_id,
            permanently_lost_device_ids,
            response,
        })
    }

    pub fn resend_entry(
        &self,
        entry_id: String,
        target_devices: Vec<String>,
    ) -> Result<ResendEntryOutcome, BindingError> {
        self.request(|response| WorkerCommand::ResendEntry {
            entry_id,
            target_devices,
            response,
        })
    }

    pub fn leave_space(&self) -> Result<(), BindingError> {
        self.request(|response| WorkerCommand::LeaveSpace { response })
    }

    pub fn lifecycle_state(&self) -> Result<BindingEngineState, BindingError> {
        let commands = self.lifecycle_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::LifecycleState { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        receive_lifecycle_result(result, LIFECYCLE_TRANSITION_DEADLINE)
    }

    pub fn create_space(
        &self,
        device_name: Option<String>,
        passphrase: String,
    ) -> Result<SpaceCreated, BindingError> {
        let passphrase = Zeroizing::new(passphrase);
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::CreateSpace {
                device_name,
                passphrase,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn issue_invitation(&self) -> Result<InvitationIssued, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::IssueInvitation { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn join_space(
        &self,
        invitation_code: String,
        device_name: Option<String>,
        passphrase: String,
        preserve_unreadable_history: bool,
    ) -> Result<SpaceJoined, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::JoinSpace {
                invitation_code: Zeroizing::new(invitation_code),
                device_name,
                passphrase: Zeroizing::new(passphrase),
                preserve_unreadable_history,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn send_text(
        &self,
        text: String,
        target_devices: Vec<String>,
    ) -> Result<SendReport, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::SendText {
                text: Zeroizing::new(text),
                target_devices,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn send_image(
        &self,
        bytes: Vec<u8>,
        mime_type: String,
        target_devices: Vec<String>,
    ) -> Result<SendReport, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::SendImage {
                bytes: Zeroizing::new(bytes),
                mime_type,
                target_devices,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn send_files(
        &self,
        file_handles: Vec<String>,
        target_devices: Vec<String>,
    ) -> Result<SendReport, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::SendFiles {
                file_handles: Zeroizing::new(file_handles),
                target_devices,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn capture_current_clipboard(&self) -> Result<Option<String>, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::CaptureCurrentClipboard { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn observe_clipboard_change(
        &self,
        dispatch: bool,
    ) -> Result<Option<SendReport>, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::ObserveClipboardChange { dispatch, response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn restore_clipboard(
        &self,
        entry_id: String,
        mode: BindingClipboardRestoreMode,
    ) -> Result<BindingClipboardRestoreOutcome, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RestoreClipboard {
                entry_id,
                mode,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn query_active_clipboard(&self) -> Result<Option<ActiveClipboard>, BindingError> {
        self.request(|response| WorkerCommand::QueryActiveClipboard { response })
    }

    pub fn export_entry(
        &self,
        entry_id: String,
        destination_handle: String,
    ) -> Result<(), BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::ExportEntry {
                entry_id,
                destination_handle: Zeroizing::new(destination_handle),
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn shutdown(&self, deadline_ms: u64) -> Result<(), BindingError> {
        self.shutdown_inner(Duration::from_millis(deadline_ms), true)
    }

    pub fn suspend(&self) -> Result<(), BindingError> {
        let commands = self.lifecycle_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::Suspend { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        receive_lifecycle_result(result, LIFECYCLE_TRANSITION_DEADLINE)?
    }

    pub fn resume(&self) -> Result<(), BindingError> {
        let commands = self.lifecycle_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::Resume { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        receive_lifecycle_result(result, LIFECYCLE_TRANSITION_DEADLINE)?
    }

    pub fn next_event(&self, timeout_ms: u64) -> Option<BindingEvent> {
        self.events.next(Duration::from_millis(timeout_ms))
    }
}

impl MobileEngine {
    fn request<T>(
        &self,
        command: impl FnOnce(mpsc::Sender<Result<T, BindingError>>) -> WorkerCommand,
    ) -> Result<T, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(command(response))
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    fn command_sender(
        &self,
    ) -> Result<tokio::sync::mpsc::UnboundedSender<WorkerCommand>, BindingError> {
        lock(&self.commands)
            .as_ref()
            .cloned()
            .ok_or(BindingError::AlreadyStopped)
    }

    fn lifecycle_sender(
        &self,
    ) -> Result<tokio::sync::mpsc::UnboundedSender<WorkerCommand>, BindingError> {
        lock(&self.lifecycle_commands)
            .as_ref()
            .cloned()
            .ok_or(BindingError::AlreadyStopped)
    }

    fn shutdown_inner(&self, deadline: Duration, join: bool) -> Result<(), BindingError> {
        let started_at = Instant::now();
        let request_sender = lock(&self.commands).take();
        let lifecycle_sender = lock(&self.lifecycle_commands)
            .take()
            .ok_or(BindingError::AlreadyStopped)?;
        let (response, result) = mpsc::channel();
        let shutdown_result = lifecycle_sender
            .send(WorkerCommand::Shutdown { deadline, response })
            .map_err(|_| BindingError::RuntimeUnavailable)
            .and_then(|()| {
                drop(request_sender);
                result
                    .recv_timeout(deadline.saturating_sub(started_at.elapsed()))
                    .map_err(|_| BindingError::RuntimeUnavailable)?
            });
        let join_result = if join {
            self.join_worker(deadline.saturating_sub(started_at.elapsed()))
        } else {
            Ok(())
        };
        shutdown_result.and(join_result)
    }

    fn join_worker(&self, deadline: Duration) -> Result<(), BindingError> {
        if let Some(worker) = lock(&self.worker).take() {
            let (finished, completion) = mpsc::channel();
            std::thread::Builder::new()
                .name("uc-engine-uniffi-reaper".to_owned())
                .spawn(move || {
                    let result = worker.join().map_err(|_| BindingError::RuntimeUnavailable);
                    let _ = finished.send(result);
                })
                .map_err(|_| BindingError::RuntimeUnavailable)?;
            completion
                .recv_timeout(deadline)
                .map_err(|_| BindingError::RuntimeUnavailable)??;
        }
        Ok(())
    }
}

impl Drop for MobileEngine {
    fn drop(&mut self) {
        let _ = self.shutdown_inner(Duration::from_secs(5), true);
    }
}

fn run_worker(
    config: EngineConfig,
    host: HostCapabilities,
    requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    lifecycle_requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    events: Arc<EventQueue>,
    started: mpsc::Sender<Result<(), BindingError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = started.send(Err(BindingError::RuntimeUnavailable));
            return;
        }
    };
    runtime.block_on(run_worker_loop(
        config,
        host,
        requests,
        lifecycle_requests,
        events,
        started,
    ));
}

async fn run_worker_loop(
    config: EngineConfig,
    host: HostCapabilities,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    mut lifecycle_requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    events: Arc<EventQueue>,
    started: mpsc::Sender<Result<(), BindingError>>,
) {
    let (engine, mut engine_events) = match Engine::start(config, host).await {
        Ok(started_engine) => started_engine,
        Err(error) => {
            let _ = started.send(Err(error.into()));
            return;
        }
    };
    if started.send(Ok(())).is_err() {
        let _ = engine.shutdown(Duration::ZERO).await;
        return;
    }
    let engine = Arc::new(engine);

    let event_task = tokio::spawn(async move {
        while let Some(event) = engine_events.next().await {
            events.push(map_engine_event(event));
        }
        events.close();
    });

    let mut shutdown_response = None;

    'worker: loop {
        let command = tokio::select! {
            biased;
            command = lifecycle_requests.recv() => command,
            command = requests.recv() => command,
        };
        let Some(command) = command else { break };
        match command {
            WorkerCommand::RecoverSession {
                allow_secure_storage_unlock,
                response,
            } => {
                if engine.lifecycle_state().await == uc_engine::EngineState::Suspended {
                    let _ = response.send(Ok(SessionRecovery {
                        unlocked: false,
                        resumed: false,
                    }));
                    continue;
                }
                let recovery_engine = Arc::clone(&engine);
                let mut recovery = tokio::spawn(async move {
                    recovery_engine
                        .execute(Operation::RecoverSession(RecoverSessionInput {
                            allow_secure_storage_unlock,
                        }))
                        .await
                        .map_err(BindingError::from)
                        .and_then(map_session_recovery)
                });
                loop {
                    tokio::select! {
                        result = &mut recovery => {
                            let result = result
                                .map_err(|_| BindingError::RuntimeUnavailable)
                                .and_then(|result| result);
                            let _ = response.send(result);
                            break;
                        }
                        lifecycle = lifecycle_requests.recv() => {
                            match lifecycle {
                                Some(WorkerCommand::Suspend { response: suspend_response }) => {
                                    let result = complete_recovery_after_lifecycle(
                                        &mut recovery,
                                        async {
                                            engine.suspend().await.map_err(BindingError::from)
                                        },
                                    )
                                    .await
                                    .map(|_| ());
                                    let suspended = result.is_ok();
                                    if suspended {
                                        let _ = response.send(Ok(SessionRecovery {
                                            unlocked: false,
                                            resumed: false,
                                        }));
                                        let _ = suspend_response.send(Ok(()));
                                        break;
                                    }
                                    let _ = suspend_response.send(result);
                                }
                                Some(WorkerCommand::LifecycleState { response }) => {
                                    let _ = response.send(map_engine_state(engine.lifecycle_state().await));
                                }
                                Some(WorkerCommand::Shutdown { deadline, response: shutdown }) => {
                                    let result = complete_recovery_after_lifecycle(
                                        &mut recovery,
                                        async {
                                            engine
                                                .shutdown(deadline)
                                                .await
                                                .map_err(BindingError::from)
                                        },
                                    )
                                    .await
                                    .map(|_| ());
                                    let _ = response.send(Err(BindingError::RuntimeUnavailable));
                                    shutdown_response = Some((shutdown, result));
                                    break 'worker;
                                }
                                Some(WorkerCommand::Resume { response }) => {
                                    let result = engine.resume().await.map_err(BindingError::from);
                                    let _ = response.send(result);
                                }
                                Some(_) => {}
                                None => {
                                    let _ = response.send(Err(BindingError::RuntimeUnavailable));
                                    break 'worker;
                                }
                            }
                        }
                    }
                }
            }
            WorkerCommand::QueryLocalDevice { response } => {
                let result = engine
                    .execute(Operation::QueryLocalDevice)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_local_device);
                let _ = response.send(result);
            }
            WorkerCommand::RefreshPeerConnections { response } => {
                let result = engine
                    .execute(Operation::RefreshPeerConnections)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_peer_connection_refresh);
                let _ = response.send(result);
            }
            WorkerCommand::SaveCustomRelay {
                mut url,
                access_token,
                previous_url,
                response,
            } => {
                let url = std::mem::take(&mut *url);
                let result =
                    if url.is_empty() && previous_url.as_deref().unwrap_or_default().is_empty() {
                        Ok(RelaySaveResult { configured: false })
                    } else {
                        let credential = if url.is_empty() {
                            RelayCredentialEdit::Delete {
                                url: previous_url.unwrap_or_default(),
                            }
                        } else if access_token.is_empty() {
                            RelayCredentialEdit::Keep { url: url.clone() }
                        } else {
                            RelayCredentialEdit::Set {
                                url: url.clone(),
                                access_token: SecretString::new(access_token.as_str()),
                            }
                        };
                        engine
                            .execute(Operation::SaveRelay(Box::new(SaveRelayInput {
                                settings: SettingsPatch {
                                    network: Some(NetworkSettingsPatch {
                                        custom_relay_urls: Some(if url.is_empty() {
                                            Vec::new()
                                        } else {
                                            vec![url]
                                        }),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                credential,
                            })))
                            .await
                            .map_err(BindingError::from)
                            .and_then(map_relay_save_result)
                    };
                let _ = response.send(result);
            }
            WorkerCommand::RecoverNetwork { response } => {
                let result = engine
                    .execute(Operation::RecoverNetwork)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_network_recovered);
                let _ = response.send(result);
            }
            WorkerCommand::QueryNetworkRecoveryStatus { response } => {
                let result = engine
                    .execute(Operation::QueryNetworkRecoveryStatus)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_network_recovery_status);
                let _ = response.send(result);
            }
            WorkerCommand::QuerySpaceState { response } => {
                let result = engine
                    .execute(Operation::QuerySetupState)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_state);
                if let Err(error) = &result {
                    log_mobile_query_failure("query_space_state", error);
                }
                let _ = response.send(result);
            }
            WorkerCommand::ListDevices { response } => {
                let result = engine
                    .execute(Operation::ListDevices)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_devices);
                if let Err(error) = &result {
                    log_mobile_query_failure("list_devices", error);
                }
                let _ = response.send(result);
            }
            WorkerCommand::QueryMembershipConvergence { response } => {
                let result = engine
                    .execute(Operation::QueryMembershipConvergence)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_membership_convergence);
                let _ = response.send(result);
            }
            WorkerCommand::RefreshSharedDevices { response } => {
                let result = engine
                    .execute(Operation::RefreshSharedDevices)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_shared_device_refresh_started);
                let _ = response.send(result);
            }
            WorkerCommand::QuerySharedDeviceRefresh {
                request_id,
                response,
            } => {
                let result = engine
                    .execute(Operation::QuerySharedDeviceRefresh(
                        QuerySharedDeviceRefreshInput { request_id },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_shared_device_refresh);
                let _ = response.send(result);
            }
            WorkerCommand::RemoveMember {
                device_id,
                response,
            } => {
                let result = engine
                    .execute(Operation::RemoveMember(RemoveMemberInput { device_id }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_member_removed);
                let _ = response.send(result);
            }
            WorkerCommand::SecureRemoveLegacyMember {
                device_id,
                response,
            } => {
                let result = engine
                    .execute(Operation::SecureRemoveLegacyMember(RemoveMemberInput {
                        device_id,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_legacy_member_removal);
                let _ = response.send(result);
            }
            WorkerCommand::QueryMemberRevocation {
                revocation_id,
                response,
            } => {
                let result = engine
                    .execute(Operation::QueryMemberRevocation(
                        QueryMemberRevocationInput { revocation_id },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_member_revocation_status);
                if let Err(error) = &result {
                    log_mobile_query_failure("query_current_member_revocation", error);
                }
                let _ = response.send(result);
            }
            WorkerCommand::QueryCurrentMemberRevocation { response } => {
                let result = engine
                    .execute(Operation::QueryCurrentMemberRevocation)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_member_revocation_status);
                let _ = response.send(result);
            }
            WorkerCommand::ContinueMemberRevocation {
                revocation_id,
                permanently_lost_device_ids,
                response,
            } => {
                let result = engine
                    .execute(Operation::ContinueMemberRevocation(
                        ContinueMemberRevocationInput {
                            revocation_id,
                            permanently_lost_device_ids,
                        },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_required_member_revocation_status);
                let _ = response.send(result);
            }
            WorkerCommand::ResendEntry {
                entry_id,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::ResendEntry(ResendEntryInput {
                        entry_id,
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_resend_outcome);
                let _ = response.send(result);
            }
            WorkerCommand::LeaveSpace { response } => {
                let result = engine
                    .execute(Operation::FactoryResetSpace)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_left);
                let _ = response.send(result);
            }
            WorkerCommand::LifecycleState { response } => {
                let _ = response.send(map_engine_state(engine.lifecycle_state().await));
            }
            WorkerCommand::CreateSpace {
                device_name,
                passphrase,
                response,
            } => {
                let result = engine
                    .execute(Operation::CreateSpace(CreateSpaceInput {
                        device_name,
                        passphrase: SecretString::new(passphrase.as_str()),
                        passphrase_confirmation: SecretString::new(passphrase.as_str()),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_created);
                let _ = response.send(result);
            }
            WorkerCommand::IssueInvitation { response } => {
                let result = engine
                    .execute(Operation::IssueInvitation)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_invitation_issued);
                let _ = response.send(result);
            }
            WorkerCommand::JoinSpace {
                mut invitation_code,
                device_name,
                passphrase,
                preserve_unreadable_history,
                response,
            } => {
                let result = engine
                    .execute(Operation::JoinSpace(JoinSpaceInput {
                        invitation_code: std::mem::take(&mut *invitation_code),
                        device_name,
                        passphrase: SecretString::new(passphrase.as_str()),
                        preserve_unreadable_history,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_joined);
                let _ = response.send(result);
            }
            WorkerCommand::SendText {
                mut text,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::SendText(SendTextInput {
                        text: std::mem::take(&mut *text),
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_send_report);
                let _ = response.send(result);
            }
            WorkerCommand::SendImage {
                mut bytes,
                mime_type,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::SendImage(SendImageInput {
                        bytes: std::mem::take(&mut *bytes),
                        mime_type,
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_send_report);
                let _ = response.send(result);
            }
            WorkerCommand::SendFiles {
                file_handles,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::SendFiles(SendFilesInput {
                        files: file_handles
                            .iter()
                            .map(|handle| HostFileHandle::new(handle.to_owned()))
                            .collect(),
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_send_report);
                let _ = response.send(result);
            }
            WorkerCommand::CaptureCurrentClipboard { response } => {
                let result = engine
                    .execute(Operation::CaptureCurrentClipboard)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_clipboard_captured);
                let _ = response.send(result);
            }
            WorkerCommand::ObserveClipboardChange { dispatch, response } => {
                let result = engine
                    .execute(Operation::ObserveClipboardChange(
                        ObserveClipboardChangeInput { dispatch },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_clipboard_change_observed);
                let _ = response.send(result);
            }
            WorkerCommand::RestoreClipboard {
                entry_id,
                mode,
                response,
            } => {
                let result = engine
                    .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                        entry_id,
                        mode: map_restore_mode(mode),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_clipboard_restored);
                let _ = response.send(result);
            }
            WorkerCommand::QueryActiveClipboard { response } => {
                let result = engine
                    .execute(Operation::QueryActiveClipboard)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_active_clipboard);
                let _ = response.send(result);
            }
            WorkerCommand::ExportEntry {
                entry_id,
                destination_handle,
                response,
            } => {
                let result = engine
                    .execute(Operation::ExportEntry(ExportEntryInput {
                        entry_id,
                        destination: HostFileHandle::new(destination_handle.as_str()),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_entry_exported);
                let _ = response.send(result);
            }
            WorkerCommand::Suspend { response } => {
                let result = engine.suspend().await.map_err(BindingError::from);
                let _ = response.send(result);
            }
            WorkerCommand::Resume { response } => {
                let result = engine.resume().await.map_err(BindingError::from);
                let _ = response.send(result);
            }
            WorkerCommand::Shutdown { deadline, response } => {
                let result = engine.shutdown(deadline).await.map_err(BindingError::from);
                shutdown_response = Some((response, result));
                break;
            }
        }
    }
    if shutdown_response.is_none() {
        let _ = engine.shutdown(Duration::ZERO).await;
    }
    let _ = event_task.await;
    if let Some((response, result)) = shutdown_response {
        let _ = response.send(result);
    }
}

async fn complete_recovery_after_lifecycle<T>(
    recovery: &mut tokio::task::JoinHandle<T>,
    lifecycle: impl Future<Output = Result<(), BindingError>>,
) -> Result<T, BindingError> {
    let lifecycle_result = lifecycle.await;
    let recovery_result = recovery.await.map_err(|_| BindingError::RuntimeUnavailable);
    lifecycle_result?;
    recovery_result
}

fn receive_lifecycle_result<T>(
    result: mpsc::Receiver<T>,
    deadline: Duration,
) -> Result<T, BindingError> {
    result
        .recv_timeout(deadline)
        .map_err(|_| BindingError::RuntimeUnavailable)
}

macro_rules! map_enum {
    ($name:ident, $($from_path:ident)::+ => $($to_path:ident)::+, $($variant:ident),+ $(,)?) => {
        fn $name(value: $($from_path)::+) -> $($to_path)::+ {
            type FromEnum = $($from_path)::+;
            type ToEnum = $($to_path)::+;
            match value {
                $(FromEnum::$variant => ToEnum::$variant,)+
            }
        }
    };
}

macro_rules! unpack_operation {
    ($result:expr, $variant:pat => $construct:expr) => {
        match $result {
            $variant => Ok($construct),
            _ => Err(BindingError::UnexpectedResult),
        }
    };
}

fn map_engine_event(event: uc_engine::EngineEvent) -> BindingEvent {
    match event {
        uc_engine::EngineEvent::StateChanged { state } => BindingEvent::StateChanged {
            state: map_engine_state(state),
        },
        uc_engine::EngineEvent::OperationFinished {
            operation_id,
            terminal,
        } => {
            let (terminal, failure) = match terminal {
                uc_engine::OperationTerminal::Succeeded => {
                    (BindingOperationTerminal::Succeeded, None)
                }
                uc_engine::OperationTerminal::Failed(error) => (
                    BindingOperationTerminal::Failed,
                    Some(BindingFailure::from(error)),
                ),
                uc_engine::OperationTerminal::Cancelled => {
                    (BindingOperationTerminal::Cancelled, None)
                }
            };
            BindingEvent::OperationFinished {
                operation_id,
                terminal,
                failure,
            }
        }
        uc_engine::EngineEvent::LifecycleFailed { action, error } => {
            BindingEvent::LifecycleFailed {
                action: map_lifecycle_action(action),
                failure: BindingFailure::from(error),
            }
        }
        uc_engine::EngineEvent::RefreshRequired { reason } => BindingEvent::RefreshRequired {
            reason: map_refresh_reason(reason),
        },
        uc_engine::EngineEvent::Fatal { error } => BindingEvent::Fatal {
            failure: BindingFailure::from(error),
        },
        uc_engine::EngineEvent::IncomingEntry(event) => BindingEvent::IncomingEntry {
            entry_id: event.entry_id,
            attempt_id: event.attempt_id,
            preview: event.preview,
            origin: map_clipboard_origin(event.origin),
        },
        uc_engine::EngineEvent::IncomingPending(event) => BindingEvent::IncomingPending {
            entry_id: event.entry_id,
            attempt_id: event.attempt_id,
            from_device: event.from_device,
            total_bytes: event.total_bytes,
            filenames: event.filenames,
        },
        uc_engine::EngineEvent::ReceiveAttemptStateChanged(event) => {
            BindingEvent::ReceiveAttemptStateChanged {
                entry_id: event.entry_id,
                attempt_id: event.attempt_id,
                state: event.state,
            }
        }
        uc_engine::EngineEvent::DeliveryStatusChanged(event) => {
            BindingEvent::DeliveryStatusChanged {
                entry_id: event.entry_id,
                target_device_id: event.target_device_id,
            }
        }
        uc_engine::EngineEvent::PeerPresenceChanged(event) => BindingEvent::PeerPresenceChanged {
            device_id: event.device_id,
            state: event.state,
            at_ms: event.at_ms,
        },
        uc_engine::EngineEvent::MemberRevocationChanged(summary) => {
            BindingEvent::MemberRevocationChanged {
                revocation: map_member_revocation_summary(summary),
            }
        }
        uc_engine::EngineEvent::SharedDeviceRefreshChanged(summary) => {
            BindingEvent::SharedDeviceRefreshChanged {
                refresh: map_shared_device_refresh_summary(summary),
            }
        }
        uc_engine::EngineEvent::TransferProgress(event) => BindingEvent::TransferProgress {
            transfer_id: event.transfer_id,
            entry_id: event.entry_id,
            attempt_id: event.attempt_id,
            peer_id: event.peer_id,
            direction: map_transfer_direction(event.direction),
            completed_bytes: event.completed_bytes,
            total_bytes: event.total_bytes,
        },
        uc_engine::EngineEvent::TransferStatusChanged(event) => {
            BindingEvent::TransferStatusChanged {
                transfer_id: event.transfer_id,
                entry_id: event.entry_id,
                attempt_id: event.attempt_id,
                status: event.status,
                reason: event.reason,
            }
        }
        uc_engine::EngineEvent::ActiveClipboardChanged(event) => {
            BindingEvent::ActiveClipboardChanged {
                snapshot_hash: event.snapshot_hash,
                entry_id: event.entry_id,
                activated_at_ms: event.activated_at_ms,
                activated_by: event.activated_by,
            }
        }
        uc_engine::EngineEvent::NetworkRecoveryChanged(status) => {
            BindingEvent::NetworkRecoveryChanged {
                phase: recovery_phase(status.phase).to_owned(),
                retryable: status.retryable,
                next_retry_in_ms: status.next_retry_in_ms,
            }
        }
        other => BindingEvent::Changed {
            kind: other.kind().to_owned(),
        },
    }
}

map_enum!(map_engine_state, uc_engine::EngineState => BindingEngineState,
    Running, Quiescing, Quiesced, Suspended, ShuttingDown, Stopped,
);

map_enum!(map_invitation_availability, uc_engine::InvitationAvailability => InvitationAvailability,
    CrossNetwork, SameLocalNetwork,
);

map_enum!(map_membership_convergence_state, uc_engine::MembershipConvergenceStateSummary => MembershipConvergenceState,
    Complete, Converging, WaitingForUpgrade, Blocked,
);

map_enum!(map_shared_device_refresh_phase, uc_engine::SharedDeviceRefreshPhaseSummary => SharedDeviceRefreshPhase,
    Started, Discovering, Connecting, RoundCompleted,
);

map_enum!(map_shared_device_refresh_device_state, uc_engine::SharedDeviceRefreshDeviceStateSummary => SharedDeviceRefreshDeviceState,
    Discovered, Connecting, Connected, AlreadyPresent, WaitingForPeer, WaitingForUpdate, VersionIncompatible, Rejected,
);

map_enum!(map_member_revocation_outcome, uc_engine::MemberRevocationOutcome => MemberRevocationOutcome,
    LocalOnly, Recovering, Applied, Complete, RecoveryRequired,
);

map_enum!(map_legacy_bootstrap_outcome, uc_engine::LegacyBootstrapOutcome => LegacyMemberRemovalOutcome,
    AwaitingReadmission, Complete, RecoveryRequired,
);

map_enum!(map_clipboard_origin, uc_engine::ClipboardOriginSummary => BindingClipboardOrigin,
    Local, Remote,
);

map_enum!(map_lifecycle_action, uc_engine::LifecycleAction => BindingLifecycleAction,
    Suspend, Resume,
);

map_enum!(map_refresh_reason, uc_engine::RefreshReason => BindingRefreshReason,
    ConsumerLagged, StateInvalidated,
);

map_enum!(map_transfer_direction, uc_engine::TransferDirectionSummary => BindingTransferDirection,
    Sending, Receiving,
);

map_enum!(map_entry_not_resendable_reason, uc_engine::EntryNotResendableReason => EntryNotResendableReason,
    RemoteOrigin, PayloadLost,
);

map_enum!(map_restore_mode, BindingClipboardRestoreMode => ClipboardRestoreMode,
    Standard, PlainText, FilePaths,
);

fn map_space_created(result: OperationResult) -> Result<SpaceCreated, BindingError> {
    unpack_operation!(result, OperationResult::SpaceCreated {
        space_id,
        self_device_id,
        identity_fingerprint,
    } => SpaceCreated {
        space_id,
        self_device_id,
        identity_fingerprint,
    })
}

fn map_space_state(result: OperationResult) -> Result<SpaceState, BindingError> {
    unpack_operation!(result, OperationResult::SetupState(state) => SpaceState {
        has_completed: state.has_completed,
        space_id: state.space_id,
        current_invitation: state.current_invitation.map(|invitation| SpaceInvitation {
            invitation_code: invitation.invitation_code,
            expires_at_ms: invitation.expires_at_ms,
        }),
        device_name: state.device_name,
    })
}

fn map_devices(result: OperationResult) -> Result<Vec<Device>, BindingError> {
    unpack_operation!(result, OperationResult::Devices(devices) => devices
        .into_iter()
        .map(|device| Device {
            device_id: device.device_id,
            display_name: device.display_name,
            is_local: device.is_local,
            online: device.online,
        })
        .collect())
}

fn map_membership_convergence(
    result: OperationResult,
) -> Result<MembershipConvergence, BindingError> {
    unpack_operation!(result, OperationResult::MembershipConvergence(summary) => MembershipConvergence {
        state: map_membership_convergence_state(summary.state),
        pending_count: summary.pending_count,
        waiting_for_peer_count: summary.waiting_for_peer_count,
        waiting_for_update_count: summary.waiting_for_update_count,
        version_incompatible_count: summary.version_incompatible_count,
        blocked_count: summary.blocked_count,
        rejected_count: summary.rejected_count,
    })
}

fn map_shared_device_refresh_started(
    result: OperationResult,
) -> Result<SharedDeviceRefreshStarted, BindingError> {
    unpack_operation!(result, OperationResult::SharedDeviceRefreshStarted(summary) => SharedDeviceRefreshStarted {
        request_id: summary.request_id,
    })
}

fn map_shared_device_refresh(
    result: OperationResult,
) -> Result<Option<SharedDeviceRefresh>, BindingError> {
    unpack_operation!(result, OperationResult::SharedDeviceRefresh(summary) => summary
        .map(map_shared_device_refresh_summary))
}

fn map_shared_device_refresh_summary(
    summary: uc_engine::SharedDeviceRefreshSummary,
) -> SharedDeviceRefresh {
    SharedDeviceRefresh {
        request_id: summary.request_id,
        phase: map_shared_device_refresh_phase(summary.phase),
        devices: summary
            .devices
            .into_iter()
            .map(|device| SharedDeviceRefreshDevice {
                device_id: device.device_id,
                display_name: device.display_name,
                state: map_shared_device_refresh_device_state(device.state),
            })
            .collect(),
        total_count: summary.total_count,
        discovered_count: summary.discovered_count,
        connecting_count: summary.connecting_count,
        connected_count: summary.connected_count,
        already_present_count: summary.already_present_count,
        waiting_for_peer_count: summary.waiting_for_peer_count,
        waiting_for_update_count: summary.waiting_for_update_count,
        version_incompatible_count: summary.version_incompatible_count,
        rejected_count: summary.rejected_count,
        unavailable_source_count: summary.unavailable_source_count,
    }
}

fn map_member_removed(result: OperationResult) -> Result<MemberRevocationResult, BindingError> {
    unpack_operation!(result, OperationResult::MemberRemoved(summary) => map_member_revocation_summary(summary))
}

fn map_legacy_member_removal(
    result: OperationResult,
) -> Result<LegacyMemberRemovalResult, BindingError> {
    unpack_operation!(result, OperationResult::LegacyMemberRemoval(summary) => LegacyMemberRemovalResult {
        bootstrap_id: summary.bootstrap_id,
        outcome: map_legacy_bootstrap_outcome(summary.outcome),
        pending_readmission: summary.pending_readmission,
    })
}

fn map_member_revocation_status(
    result: OperationResult,
) -> Result<Option<MemberRevocationResult>, BindingError> {
    unpack_operation!(result, OperationResult::MemberRevocationStatus(summary) => summary
        .map(map_member_revocation_summary))
}

fn map_required_member_revocation_status(
    result: OperationResult,
) -> Result<MemberRevocationResult, BindingError> {
    map_member_revocation_status(result)?.ok_or(BindingError::UnexpectedResult)
}

fn map_member_revocation_summary(
    summary: uc_engine::MemberRevocationSummary,
) -> MemberRevocationResult {
    MemberRevocationResult {
        revocation_id: summary.revocation_id,
        outcome: map_member_revocation_outcome(summary.outcome),
        pending_recipients: summary.pending_recipients,
        removed_device_ids: summary.removed_device_ids,
        pending_recipient_device_ids: summary.pending_recipient_device_ids,
        updated_at_ms: summary.updated_at_ms,
    }
}

fn map_resend_outcome(result: OperationResult) -> Result<ResendEntryOutcome, BindingError> {
    unpack_operation!(result, OperationResult::EntryResent(outcome) => match outcome {
        uc_engine::ResendEntryOutcome::Completed(report) => ResendEntryOutcome::Completed {
            accepted: count_to_u64(report.accepted)?,
            duplicate: count_to_u64(report.duplicate)?,
            offline: count_to_u64(report.offline)?,
            errored: count_to_u64(report.errored)?,
            pending: count_to_u64(report.pending)?,
        },
        uc_engine::ResendEntryOutcome::EntryNotFound { entry_id } => {
            ResendEntryOutcome::EntryNotFound { entry_id }
        }
        uc_engine::ResendEntryOutcome::EntryNotResendable { entry_id, reason } => {
            ResendEntryOutcome::EntryNotResendable {
                entry_id,
                reason: map_entry_not_resendable_reason(reason),
            }
        }
        uc_engine::ResendEntryOutcome::TargetNotTrusted { device_id } => {
            ResendEntryOutcome::TargetNotTrusted { device_id }
        }
        uc_engine::ResendEntryOutcome::NoEligibleTargets => ResendEntryOutcome::NoEligibleTargets,
    })
}

fn map_space_left(result: OperationResult) -> Result<(), BindingError> {
    unpack_operation!(result, OperationResult::SpaceFactoryReset => ())
}

fn map_session_recovery(result: OperationResult) -> Result<SessionRecovery, BindingError> {
    unpack_operation!(result, OperationResult::SessionRecovered { unlocked, resumed } => SessionRecovery {
        unlocked,
        resumed,
    })
}

fn map_local_device(result: OperationResult) -> Result<LocalDevice, BindingError> {
    unpack_operation!(result, OperationResult::LocalDevice(device) => LocalDevice {
        device_id: device.device_id,
        display_name: device.display_name,
    })
}

fn map_invitation_issued(result: OperationResult) -> Result<InvitationIssued, BindingError> {
    unpack_operation!(result, OperationResult::InvitationIssued {
        invitation_code,
        expires_at_ms,
        availability,
    } => InvitationIssued {
        invitation_code,
        expires_at_ms,
        availability: map_invitation_availability(availability),
    })
}

fn map_space_joined(result: OperationResult) -> Result<SpaceJoined, BindingError> {
    unpack_operation!(result, OperationResult::SpaceJoined {
        sponsor_device_id,
        sponsor_identity_fingerprint,
        space_id,
        self_device_id,
        self_identity_fingerprint,
        migrated_records,
        preserved_unreadable_records,
    } => SpaceJoined {
        sponsor_device_id,
        sponsor_identity_fingerprint,
        space_id,
        self_device_id,
        self_identity_fingerprint,
        migrated_records,
        preserved_unreadable_records,
    })
}

fn map_send_report(result: OperationResult) -> Result<SendReport, BindingError> {
    unpack_operation!(result, OperationResult::EntrySent(report) => SendReport {
        entry_id: report.entry_id,
        at_ms: report.at_ms,
        total_accepted: count_to_u64(report.total_accepted)?,
        total_duplicate: count_to_u64(report.total_duplicate)?,
        total_offline: count_to_u64(report.total_offline)?,
        total_errored: count_to_u64(report.total_errored)?,
        total_pending: count_to_u64(report.total_pending)?,
    })
}

fn map_peer_connection_refresh(
    result: OperationResult,
) -> Result<PeerConnectionRefresh, BindingError> {
    unpack_operation!(result, OperationResult::PeerConnectionsRefreshed(report) => PeerConnectionRefresh {
        total: u64::from(report.total),
        online: u64::from(report.online),
        offline: u64::from(report.offline),
        errors: u64::from(report.errors),
    })
}

fn map_relay_save_result(result: OperationResult) -> Result<RelaySaveResult, BindingError> {
    match result {
        OperationResult::RelaySaved(SaveRelayOutcome::Saved { settings, .. }) => {
            Ok(RelaySaveResult {
                configured: !settings.network.custom_relay_urls.is_empty(),
            })
        }
        OperationResult::RelaySaved(SaveRelayOutcome::Rejected { .. }) => {
            Err(BindingError::Engine {
                code: 0,
                category: BindingErrorCategory::InvalidInput,
                retryable: false,
            })
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_network_recovered(result: OperationResult) -> Result<(), BindingError> {
    unpack_operation!(result, OperationResult::NetworkRecovered => ())
}

fn map_network_recovery_status(
    result: OperationResult,
) -> Result<NetworkRecoveryStatus, BindingError> {
    unpack_operation!(result, OperationResult::NetworkRecoveryStatus(status) => NetworkRecoveryStatus {
        phase: recovery_phase(status.phase).to_string(),
        retryable: status.retryable,
        next_retry_in_ms: status.next_retry_in_ms,
    })
}

fn recovery_phase(phase: uc_engine::NetworkRecoveryPhaseSummary) -> &'static str {
    match phase {
        uc_engine::NetworkRecoveryPhaseSummary::Idle => "idle",
        uc_engine::NetworkRecoveryPhaseSummary::Recovering => "recovering",
        uc_engine::NetworkRecoveryPhaseSummary::RetryScheduled => "retry_scheduled",
        uc_engine::NetworkRecoveryPhaseSummary::Failed => "failed",
    }
}

fn map_clipboard_change_observed(
    result: OperationResult,
) -> Result<Option<SendReport>, BindingError> {
    match result {
        OperationResult::ClipboardChangeObserved { report } => report
            .map(|report| map_send_report(OperationResult::EntrySent(report)))
            .transpose(),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_clipboard_captured(result: OperationResult) -> Result<Option<String>, BindingError> {
    unpack_operation!(result, OperationResult::ClipboardCaptured { entry_id } => entry_id)
}

fn map_clipboard_restored(
    result: OperationResult,
) -> Result<BindingClipboardRestoreOutcome, BindingError> {
    match result {
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::Restored) => {
            Ok(BindingClipboardRestoreOutcome::Restored)
        }
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::PayloadUnavailable {
            ..
        }) => Ok(BindingClipboardRestoreOutcome::PayloadUnavailable),
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::NotApplicable { .. }) => {
            Ok(BindingClipboardRestoreOutcome::NotApplicable)
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_active_clipboard(result: OperationResult) -> Result<Option<ActiveClipboard>, BindingError> {
    unpack_operation!(result, OperationResult::ActiveClipboard(active) => active.map(|active| ActiveClipboard {
        entry_id: active.entry_id,
        activated_by: active.activated_by,
    }))
}

fn map_entry_exported(result: OperationResult) -> Result<(), BindingError> {
    unpack_operation!(result, OperationResult::EntryExported => ())
}

fn count_to_u64(value: usize) -> Result<u64, BindingError> {
    u64::try_from(value).map_err(|_| BindingError::UnexpectedResult)
}

fn host_capabilities(
    host: Arc<dyn BindingHost>,
    analytics: Option<(Arc<dyn BindingAnalyticsHost>, BindingAnalyticsContext)>,
) -> Result<HostCapabilities, BindingError> {
    let cache_directory = host_path(host.cache_directory())?;
    let directories = HostDirectories::new(
        host_path(host.private_data_directory())?,
        cache_directory.clone(),
        host_path(host.temporary_directory())?,
        cache_directory.join("logs"),
    );
    for directory in [
        directories.private_data(),
        directories.cache(),
        directories.temporary(),
        directories.logs(),
    ] {
        std::fs::create_dir_all(directory).map_err(|_| BindingError::HostIo)?;
    }
    let capabilities = HostCapabilities::new(
        directories,
        Box::new(BindingSecureStorage {
            host: Arc::clone(&host),
        }),
        Box::new(BindingClipboard {
            host: Arc::clone(&host),
        }),
        Box::new(BindingFiles { host }),
    );
    let Some((analytics, context)) = analytics else {
        return Ok(capabilities);
    };
    let adapter = Arc::new(BindingAnalyticsAdapter::new(
        analytics,
        analytics_event_context(context),
    ));
    let sink: Arc<dyn AnalyticsPort> = adapter.clone();
    let identity: Arc<dyn AnalyticsIdentityPort> = adapter;
    Ok(capabilities.with_analytics(sink, identity))
}

fn analytics_event_context(context: BindingAnalyticsContext) -> AnalyticsEventContext {
    AnalyticsEventContext {
        os: match context.os {
            BindingAnalyticsOs::Macos => Os::Macos,
            BindingAnalyticsOs::Windows => Os::Windows,
            BindingAnalyticsOs::Linux => Os::Linux,
            BindingAnalyticsOs::Ios => Os::Ios,
            BindingAnalyticsOs::Android => Os::Android,
            BindingAnalyticsOs::Other => Os::Other,
        },
        os_version: context.os_version,
        device_type: match context.device_type {
            BindingAnalyticsDeviceType::Mobile => DeviceType::Mobile,
            BindingAnalyticsDeviceType::Desktop => DeviceType::Desktop,
        },
        arch: context.arch,
        app_channel: context.app_channel,
    }
}

fn host_path(result: Result<String, HostBindingError>) -> Result<PathBuf, BindingError> {
    result.map(PathBuf::from).map_err(map_binding_host_error)
}

fn map_binding_host_error(error: HostBindingError) -> BindingError {
    match error {
        HostBindingError::Unavailable => BindingError::HostUnavailable,
        HostBindingError::PermissionDenied => BindingError::HostPermissionDenied,
        HostBindingError::InvalidHandle => BindingError::HostInvalidHandle,
        HostBindingError::Io => BindingError::HostIo,
    }
}

fn map_host_capability_error(error: HostBindingError) -> HostCapabilityError {
    let category = match error {
        HostBindingError::Unavailable => HostCapabilityErrorCategory::Unavailable,
        HostBindingError::PermissionDenied => HostCapabilityErrorCategory::PermissionDenied,
        HostBindingError::InvalidHandle => HostCapabilityErrorCategory::InvalidHandle,
        HostBindingError::Io => HostCapabilityErrorCategory::Io,
    };
    HostCapabilityError::new(category, "binding host callback failed")
}

struct BindingSecureStorage {
    host: Arc<dyn BindingHost>,
}

impl HostSecureStorage for BindingSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        self.host
            .secure_storage_get(key.to_owned())
            .map_err(map_host_capability_error)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.host
            .secure_storage_set(key.to_owned(), value.to_vec())
            .map_err(map_host_capability_error)
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.host
            .secure_storage_delete(key.to_owned())
            .map_err(map_host_capability_error)
    }
}

struct BindingClipboard {
    host: Arc<dyn BindingHost>,
}

impl HostClipboard for BindingClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.host
            .clipboard_read()
            .map(|snapshot| HostClipboardSnapshot {
                observed_at_ms: snapshot.observed_at_ms,
                representations: snapshot
                    .representations
                    .into_iter()
                    .map(map_clipboard_representation)
                    .collect(),
            })
            .map_err(map_host_capability_error)
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.host
            .clipboard_write(BindingClipboardSnapshot {
                observed_at_ms: snapshot.observed_at_ms,
                representations: snapshot
                    .representations
                    .into_iter()
                    .map(map_engine_clipboard_representation)
                    .collect(),
            })
            .map_err(map_host_capability_error)
    }
}

fn map_clipboard_representation(
    representation: BindingClipboardRepresentation,
) -> HostClipboardRepresentation {
    match representation {
        BindingClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        } => HostClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        },
        BindingClipboardRepresentation::File {
            format,
            handle,
            display_name,
            mime_type,
            size_bytes,
        } => HostClipboardRepresentation::File {
            format,
            handle: HostFileHandle::new(handle),
            display_name,
            mime_type,
            size_bytes,
        },
    }
}

fn map_engine_clipboard_representation(
    representation: HostClipboardRepresentation,
) -> BindingClipboardRepresentation {
    match representation {
        HostClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        } => BindingClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        },
        HostClipboardRepresentation::File {
            format,
            handle,
            display_name,
            mime_type,
            size_bytes,
        } => BindingClipboardRepresentation::File {
            format,
            handle: handle.as_str().to_owned(),
            display_name,
            mime_type,
            size_bytes,
        },
    }
}

struct BindingFiles {
    host: Arc<dyn BindingHost>,
}

impl HostFileAccess for BindingFiles {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        self.host
            .file_metadata(handle.as_str().to_owned())
            .map(|metadata: BindingFileMetadata| HostFileMetadata {
                display_name: metadata.display_name,
                size_bytes: metadata.size_bytes,
                mime_type: metadata.mime_type,
            })
            .map_err(map_host_capability_error)
    }

    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        self.host
            .file_read_chunk(handle.as_str().to_owned(), offset, max_bytes)
            .map_err(map_host_capability_error)
    }

    fn write_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        self.host
            .file_write_chunk(handle.as_str().to_owned(), offset, bytes.to_vec())
            .map_err(map_host_capability_error)
    }

    fn finish_write(&self, handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        self.host
            .file_finish_write(handle.as_str().to_owned())
            .map_err(map_host_capability_error)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_revocation_methods_and_events_share_the_complete_snapshot() {
        let _query: fn(&MobileEngine) -> Result<Option<MemberRevocationResult>, BindingError> =
            MobileEngine::query_current_member_revocation;
        let _continue: fn(
            &MobileEngine,
            String,
            Vec<String>,
        ) -> Result<MemberRevocationResult, BindingError> =
            MobileEngine::continue_member_revocation;
        let event = map_engine_event(uc_engine::EngineEvent::MemberRevocationChanged(
            uc_engine::MemberRevocationSummary {
                revocation_id: Some("revocation-1".into()),
                outcome: uc_engine::MemberRevocationOutcome::Applied,
                pending_recipients: 1,
                removed_device_ids: vec!["removed-1".into()],
                pending_recipient_device_ids: vec!["waiting-1".into()],
                updated_at_ms: 42,
            },
        ));

        assert_eq!(
            event,
            BindingEvent::MemberRevocationChanged {
                revocation: MemberRevocationResult {
                    revocation_id: Some("revocation-1".into()),
                    outcome: MemberRevocationOutcome::Applied,
                    pending_recipients: 1,
                    removed_device_ids: vec!["removed-1".into()],
                    pending_recipient_device_ids: vec!["waiting-1".into()],
                    updated_at_ms: 42,
                },
            }
        );
    }

    #[test]
    fn recovering_member_revocation_survives_the_mobile_binding() {
        let result = map_member_revocation_summary(uc_engine::MemberRevocationSummary {
            revocation_id: Some("revocation-prepared".into()),
            outcome: uc_engine::MemberRevocationOutcome::Recovering,
            pending_recipients: 0,
            removed_device_ids: vec!["removed-1".into()],
            pending_recipient_device_ids: Vec::new(),
            updated_at_ms: 42,
        });

        assert_eq!(result.outcome, MemberRevocationOutcome::Recovering);
    }

    #[test]
    fn shared_device_refresh_methods_and_event_keep_the_complete_snapshot() {
        let _refresh: fn(&MobileEngine) -> Result<SharedDeviceRefreshStarted, BindingError> =
            MobileEngine::refresh_shared_devices;
        let _query: fn(&MobileEngine, String) -> Result<Option<SharedDeviceRefresh>, BindingError> =
            MobileEngine::query_shared_device_refresh;
        let summary = uc_engine::SharedDeviceRefreshSummary {
            request_id: "refresh-1".into(),
            phase: uc_engine::SharedDeviceRefreshPhaseSummary::RoundCompleted,
            devices: vec![uc_engine::SharedDeviceRefreshDeviceSummary {
                device_id: "device-c".into(),
                display_name: "Device C".into(),
                state: uc_engine::SharedDeviceRefreshDeviceStateSummary::Connected,
            }],
            total_count: 1,
            discovered_count: 0,
            connecting_count: 0,
            connected_count: 1,
            already_present_count: 0,
            waiting_for_peer_count: 0,
            waiting_for_update_count: 0,
            version_incompatible_count: 0,
            rejected_count: 0,
            unavailable_source_count: 2,
        };

        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::SharedDeviceRefreshChanged(summary)),
            BindingEvent::SharedDeviceRefreshChanged {
                refresh: SharedDeviceRefresh {
                    request_id: "refresh-1".into(),
                    phase: SharedDeviceRefreshPhase::RoundCompleted,
                    devices: vec![SharedDeviceRefreshDevice {
                        device_id: "device-c".into(),
                        display_name: "Device C".into(),
                        state: SharedDeviceRefreshDeviceState::Connected,
                    }],
                    total_count: 1,
                    discovered_count: 0,
                    connecting_count: 0,
                    connected_count: 1,
                    already_present_count: 0,
                    waiting_for_peer_count: 0,
                    waiting_for_update_count: 0,
                    version_incompatible_count: 0,
                    rejected_count: 0,
                    unavailable_source_count: 2,
                },
            }
        );
    }

    #[test]
    fn space_joined_mapping_preserves_history_counts() {
        let joined = map_space_joined(OperationResult::SpaceJoined {
            sponsor_device_id: "sponsor".into(),
            sponsor_identity_fingerprint: "sponsor-fingerprint".into(),
            space_id: "space".into(),
            self_device_id: "self".into(),
            self_identity_fingerprint: "self-fingerprint".into(),
            migrated_records: Some(4),
            preserved_unreadable_records: Some(2),
        })
        .expect("space-joined result must map");

        assert_eq!(joined.migrated_records, Some(4));
        assert_eq!(joined.preserved_unreadable_records, Some(2));
    }

    #[test]
    fn lifecycle_wait_keeps_recovery_polled_until_cancellation_settles() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime must start");

        runtime.block_on(async {
            let (started, recovery_started) = tokio::sync::oneshot::channel();
            let (cancel, cancelled) = tokio::sync::oneshot::channel();
            let (settled, recovery_settled) = tokio::sync::oneshot::channel();
            let mut recovery = tokio::spawn(async move {
                let _ = started.send(());
                let _ = cancelled.await;
                let _ = settled.send(());
                "cancelled"
            });
            recovery_started.await.expect("recovery task did not start");
            let lifecycle = async move {
                let _ = cancel.send(());
                tokio::time::timeout(Duration::from_millis(100), recovery_settled)
                    .await
                    .map_err(|_| BindingError::RuntimeUnavailable)?
                    .map_err(|_| BindingError::RuntimeUnavailable)?;
                Ok(())
            };

            let outcome = complete_recovery_after_lifecycle(&mut recovery, lifecycle)
                .await
                .expect("lifecycle and cancelled recovery must both settle");

            assert_eq!(outcome, "cancelled");
        });
    }

    #[test]
    fn shutdown_deadline_bounds_the_worker_reply_and_join() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(1));
        let worker = std::thread::spawn(move || {
            drop(requests);
            if let Some(WorkerCommand::Shutdown { response, .. }) =
                lifecycle_requests.blocking_recv()
            {
                std::thread::sleep(Duration::from_millis(200));
                let _ = response.send(Ok(()));
            }
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            events,
            worker: Mutex::new(Some(worker)),
        };
        let started_at = Instant::now();

        let result = engine.shutdown(10);

        assert!(result.is_err());
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "shutdown exceeded its end-to-end deadline"
        );
    }

    #[test]
    fn suspend_is_not_queued_behind_session_recovery() {
        let (commands, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let (recovery_started, started) = mpsc::channel();
        let events = Arc::new(EventQueue::new(1));
        let worker = std::thread::spawn(move || {
            if let Some(WorkerCommand::RecoverSession { response, .. }) = requests.blocking_recv() {
                let _ = recovery_started.send(());
                let recovery = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    let _ = response.send(Err(BindingError::RuntimeUnavailable));
                });
                if let Some(WorkerCommand::Suspend { response }) =
                    lifecycle_requests.blocking_recv()
                {
                    let _ = response.send(Ok(()));
                }
                let _ = recovery.join();
            }
        });
        let engine = Arc::new(MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            events,
            worker: Mutex::new(Some(worker)),
        });
        let recovering_engine = Arc::clone(&engine);
        let recovery = std::thread::spawn(move || recovering_engine.recover_session(true));
        started
            .recv_timeout(Duration::from_secs(1))
            .expect("recovery request did not reach the worker");
        let started_at = Instant::now();

        let result = engine.suspend();

        assert!(result.is_ok());
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "suspend waited for session recovery to finish"
        );
        let _ = recovery.join();
    }

    #[test]
    fn lifecycle_reply_wait_respects_the_supplied_deadline() {
        let (response, result) = mpsc::channel::<Result<(), BindingError>>();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = response.send(Ok(()));
        });
        let started_at = Instant::now();

        let result = receive_lifecycle_result(result, Duration::from_millis(10));

        assert!(result.is_err());
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "lifecycle reply exceeded its supplied deadline"
        );
        let _ = worker.join();
    }

    #[test]
    fn bounded_event_queue_reports_lag_and_keeps_the_latest_events() {
        let queue = EventQueue::new(2);
        queue.push(BindingEvent::Changed {
            kind: "first".to_owned(),
        });
        queue.push(BindingEvent::Changed {
            kind: "second".to_owned(),
        });
        queue.push(BindingEvent::Changed {
            kind: "third".to_owned(),
        });

        assert_eq!(
            queue.next(Duration::ZERO),
            Some(BindingEvent::RefreshRequired {
                reason: BindingRefreshReason::ConsumerLagged,
            })
        );
        assert_eq!(
            queue.next(Duration::ZERO),
            Some(BindingEvent::Changed {
                kind: "second".to_owned(),
            })
        );
        assert_eq!(
            queue.next(Duration::ZERO),
            Some(BindingEvent::Changed {
                kind: "third".to_owned(),
            })
        );
        assert_eq!(queue.next(Duration::ZERO), None);
    }

    #[test]
    fn event_queue_handles_the_largest_foreign_timeout_without_panicking() {
        let queue = EventQueue::new(1);
        queue.close();

        assert_eq!(queue.next(Duration::MAX), None);
    }

    #[test]
    fn invitation_availability_survives_the_mobile_binding() {
        for (engine_availability, binding_availability) in [
            (
                uc_engine::InvitationAvailability::CrossNetwork,
                InvitationAvailability::CrossNetwork,
            ),
            (
                uc_engine::InvitationAvailability::SameLocalNetwork,
                InvitationAvailability::SameLocalNetwork,
            ),
        ] {
            let invitation = map_invitation_issued(OperationResult::InvitationIssued {
                invitation_code: "NEVER-SHOW".to_owned(),
                expires_at_ms: 1,
                availability: engine_availability,
            })
            .expect("invitation result must map into the mobile binding");

            assert_eq!(invitation.availability, binding_availability);
        }
    }

    #[test]
    fn legacy_member_removal_survives_the_mobile_binding() {
        let _public_method: fn(
            &MobileEngine,
            String,
        ) -> Result<LegacyMemberRemovalResult, BindingError> =
            MobileEngine::secure_remove_legacy_member;
        let removal = map_legacy_member_removal(OperationResult::LegacyMemberRemoval(
            uc_engine::LegacyBootstrapSummary {
                bootstrap_id: "bootstrap-1".to_owned(),
                outcome: uc_engine::LegacyBootstrapOutcome::AwaitingReadmission,
                pending_readmission: 2,
            },
        ))
        .expect("legacy member removal must map into the mobile binding");

        assert_eq!(
            removal,
            LegacyMemberRemovalResult {
                bootstrap_id: "bootstrap-1".to_owned(),
                outcome: LegacyMemberRemovalOutcome::AwaitingReadmission,
                pending_readmission: 2,
            }
        );
    }

    #[test]
    fn lifecycle_failure_keeps_the_action_and_stable_failure() {
        let event = map_engine_event(uc_engine::EngineEvent::LifecycleFailed {
            action: uc_engine::LifecycleAction::Suspend,
            error: uc_engine::EngineError::new(
                1214,
                uc_engine::EngineErrorCategory::Unavailable,
                true,
            ),
        });

        assert_eq!(
            event,
            BindingEvent::LifecycleFailed {
                action: BindingLifecycleAction::Suspend,
                failure: BindingFailure {
                    code: 1214,
                    category: crate::BindingErrorCategory::Unavailable,
                    retryable: true,
                },
            }
        );
    }

    #[test]
    fn incoming_entry_event_keeps_history_identity_and_origin() {
        let event = map_engine_event(uc_engine::EngineEvent::IncomingEntry(
            uc_engine::IncomingEntryEvent {
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                preview: "New clipboard content".to_owned(),
                origin: uc_engine::ClipboardOriginSummary::Remote,
            },
        ));

        assert_eq!(
            event,
            BindingEvent::IncomingEntry {
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                preview: "New clipboard content".to_owned(),
                origin: BindingClipboardOrigin::Remote,
            }
        );
    }

    #[test]
    fn delivery_and_presence_events_keep_their_target_state() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::DeliveryStatusChanged(
                uc_engine::DeliveryStatusChanged {
                    entry_id: "entry-1".to_owned(),
                    target_device_id: "device-2".to_owned(),
                },
            )),
            BindingEvent::DeliveryStatusChanged {
                entry_id: "entry-1".to_owned(),
                target_device_id: "device-2".to_owned(),
            }
        );
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::PeerPresenceChanged(
                uc_engine::PeerPresenceChanged {
                    device_id: "device-2".to_owned(),
                    state: "online".to_owned(),
                    at_ms: 42,
                },
            )),
            BindingEvent::PeerPresenceChanged {
                device_id: "device-2".to_owned(),
                state: "online".to_owned(),
                at_ms: 42,
            }
        );
    }

    #[test]
    fn network_recovery_event_keeps_the_stable_status() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::NetworkRecoveryChanged(
                uc_engine::NetworkRecoveryStatusSummary {
                    phase: uc_engine::NetworkRecoveryPhaseSummary::RetryScheduled,
                    retryable: true,
                    next_retry_in_ms: Some(500),
                },
            )),
            BindingEvent::NetworkRecoveryChanged {
                phase: "retry_scheduled".to_owned(),
                retryable: true,
                next_retry_in_ms: Some(500),
            }
        );
    }

    #[test]
    fn transfer_events_keep_progress_and_terminal_details() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::TransferProgress(
                uc_engine::TransferProgress {
                    transfer_id: "transfer-1".to_owned(),
                    entry_id: Some("entry-1".to_owned()),
                    attempt_id: Some("attempt-1".to_owned()),
                    peer_id: "device-2".to_owned(),
                    direction: uc_engine::TransferDirectionSummary::Receiving,
                    completed_bytes: 64,
                    total_bytes: Some(128),
                },
            )),
            BindingEvent::TransferProgress {
                transfer_id: "transfer-1".to_owned(),
                entry_id: Some("entry-1".to_owned()),
                attempt_id: Some("attempt-1".to_owned()),
                peer_id: "device-2".to_owned(),
                direction: BindingTransferDirection::Receiving,
                completed_bytes: 64,
                total_bytes: Some(128),
            }
        );
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::TransferStatusChanged(
                uc_engine::TransferStatusChanged {
                    transfer_id: "transfer-1".to_owned(),
                    entry_id: "entry-1".to_owned(),
                    attempt_id: Some("attempt-1".to_owned()),
                    status: "completed".to_owned(),
                    reason: None,
                },
            )),
            BindingEvent::TransferStatusChanged {
                transfer_id: "transfer-1".to_owned(),
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                status: "completed".to_owned(),
                reason: None,
            }
        );
    }

    #[test]
    fn pending_receive_event_keeps_entry_and_file_summary() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::IncomingPending(
                uc_engine::IncomingPendingEvent {
                    entry_id: "entry-1".to_owned(),
                    attempt_id: Some("attempt-1".to_owned()),
                    from_device: "device-2".to_owned(),
                    total_bytes: Some(256),
                    filenames: vec!["private-name.txt".to_owned()],
                },
            )),
            BindingEvent::IncomingPending {
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                from_device: "device-2".to_owned(),
                total_bytes: Some(256),
                filenames: vec!["private-name.txt".to_owned()],
            }
        );
    }

    #[test]
    fn receive_attempt_and_active_clipboard_events_keep_refresh_identity() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::ReceiveAttemptStateChanged(
                uc_engine::ReceiveAttemptStateChanged {
                    entry_id: "entry-1".to_owned(),
                    attempt_id: "attempt-1".to_owned(),
                    state: "completed".to_owned(),
                },
            )),
            BindingEvent::ReceiveAttemptStateChanged {
                entry_id: "entry-1".to_owned(),
                attempt_id: "attempt-1".to_owned(),
                state: "completed".to_owned(),
            }
        );
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::ActiveClipboardChanged(
                uc_engine::ActiveClipboardChanged {
                    snapshot_hash: "snapshot-1".to_owned(),
                    entry_id: "entry-1".to_owned(),
                    activated_at_ms: 42,
                    activated_by: "device-2".to_owned(),
                },
            )),
            BindingEvent::ActiveClipboardChanged {
                snapshot_hash: "snapshot-1".to_owned(),
                entry_id: "entry-1".to_owned(),
                activated_at_ms: 42,
                activated_by: "device-2".to_owned(),
            }
        );
    }
}
