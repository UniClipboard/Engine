use std::collections::VecDeque;
use std::future::{pending, Future};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tracing::warn;
use uc_engine::observability::{
    AdoptOutcome, AnalyticsEventContext, AnalyticsIdentityError, AnalyticsIdentityPort,
    AnalyticsPort, DeviceType, Event, GroupIdentifyPayload, IdentifyPayload, Os, ReleaseOutcome,
};
use uc_engine::{
    ChangeEncryptionPassphraseInput, ClipboardRestoreMode, ClipboardRestoreOutcome,
    CreateSpaceInput, CustomRelayMutation, CustomRelayMutationOutcome, CustomRelayRejection,
    Engine, EngineConfig, ExportEntryInput, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot,
    HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage,
    JoinSpaceInput, ObserveClipboardChangeInput, Operation, OperationResult, RecoverSessionInput,
    RemoveMemberInput, ResendEntryInput, RestoreClipboardInput, SecretString, SendFilesInput,
    SendImageInput, SendTextInput, StartupLifecycleInput, StartupProgress,
};
use zeroize::Zeroizing;

use crate::{
    BindingAnalyticsContext, BindingAnalyticsDeviceType, BindingAnalyticsHost, BindingAnalyticsOs,
    BindingClipboardOrigin, BindingClipboardRepresentation, BindingClipboardRestoreMode,
    BindingClipboardRestoreOutcome, BindingClipboardSnapshot, BindingConfig, BindingEngineState,
    BindingError, BindingErrorCategory, BindingEvent, BindingFailure, BindingFileMetadata,
    BindingHost, BindingLifecycleAction, BindingOperationTerminal, BindingRePairingScope,
    BindingRefreshReason, BindingTransferDirection, HostBindingError,
};

const LIFECYCLE_TRANSITION_DEADLINE: Duration = Duration::from_secs(10);
mod lifecycle;
mod shutdown;
mod startup_lifecycle;
mod worker_join;
mod worker_lifecycle;
mod worker_shutdown;
pub use startup_lifecycle::MobileStartupLifecycle;
use worker_join::WorkerJoin;
use worker_lifecycle::LifecycleCommand;

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
        BindingError::ObservabilityConfigInvalid => {
            warn!(
                operation,
                error_kind = "observability_config_invalid",
                "mobile query failed"
            )
        }
        BindingError::ObservabilityConfigConflict => {
            warn!(
                operation,
                error_kind = "observability_config_conflict",
                "mobile query failed"
            )
        }
        BindingError::ObservabilityRuntimeUnavailable => {
            warn!(
                operation,
                error_kind = "observability_runtime_unavailable",
                "mobile query failed"
            )
        }
        BindingError::ObservabilityNotInstalled => {
            warn!(
                operation,
                error_kind = "observability_not_installed",
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
    pub full_invitation: String,
    pub expires_at_ms: i64,
    pub availability: InvitationAvailability,
}

impl std::fmt::Debug for InvitationIssued {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvitationIssued")
            .field("invitation_code", &"[REDACTED]")
            .field("full_invitation", &"[REDACTED]")
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
pub struct JoinedSpace {
    pub sponsor_device_id: String,
    pub sponsor_identity_fingerprint: String,
    pub space_id: String,
    pub self_device_id: String,
    pub self_identity_fingerprint: String,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum JoinSpaceRejectionReason {
    InvitationUnavailable,
    AuthenticationRejected,
    IdentityConflict,
    BaseHistoryChanged,
    JoinerHistoryAhead,
    HistoryConflict,
    CompletionInvalid,
    MembershipHistoryInvalid,
    SecurityMaterialInvalid,
    RelationshipConflict,
    ActivationStateInvalid,
    PeerUpgradeRequired,
    Cancelled,
    RemovedBeforeActivation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum JoinSpaceTerminationReason {
    Cancelled,
    Expired,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum JoinSpaceAttentionReason {
    OutcomeCannotBeProven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum JoinSpaceAttentionRecovery {
    PreserveDataAndContactSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum JoinSpaceStatus {
    Active {
        join_id: String,
        joined_space: JoinedSpace,
        peer_upgrade_required: bool,
    },
    Pending {
        join_id: String,
        target_space_id: Option<String>,
        sponsor_device_id: Option<String>,
        sponsor_identity_fingerprint: Option<String>,
        cancel_requested: bool,
        peer_upgrade_required: bool,
    },
    Processing {
        join_id: String,
        target_space_id: String,
        sponsor_device_id: String,
        sponsor_identity_fingerprint: String,
        peer_upgrade_required: bool,
    },
    NeedsAttention {
        join_id: String,
        reason: JoinSpaceAttentionReason,
        recovery: JoinSpaceAttentionRecovery,
        next_retry_at_ms: Option<i64>,
    },
    Rejected {
        join_id: String,
        reason: JoinSpaceRejectionReason,
    },
    Terminated {
        join_id: String,
        reason: JoinSpaceTerminationReason,
    },
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

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceInvitation {
    pub invitation_code: String,
    pub full_invitation: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for SpaceInvitation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpaceInvitation")
            .field("invitation_code", &"[REDACTED]")
            .field("full_invitation", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceState {
    pub has_completed: bool,
    pub re_pairing_required: bool,
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
pub enum WorkspaceConvergencePhase {
    LocallyApplied,
    Converging,
    Complete,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WorkspaceConvergenceFailureCategory {
    SpaceMismatch,
    ContinuityGap,
    IdentityMismatch,
    DigestConflict,
    Unauthorized,
    VersionIncompatible,
    NoEffectiveMembers,
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WorkspaceConvergence {
    pub phase: WorkspaceConvergencePhase,
    pub revision: u64,
    pub history_event_count: u64,
    pub effective_member_count: u64,
    pub pending_removal_decision_device_ids: Vec<String>,
    pub pending_removal_decision_event_id: Option<String>,
    pub diverged_peer_device_ids: Vec<String>,
    pub upgrade_required_peer_device_ids: Vec<String>,
    pub convergence_digest: Option<String>,
    pub removed: bool,
    pub updated_at_ms: i64,
    pub failure_category: Option<WorkspaceConvergenceFailureCategory>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EntryNotResendableReason {
    RemoteOrigin,
    PayloadLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MembershipRemovalDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceTrustChoice {
    ApplyChange,
    KeepCurrentDeviceGroup,
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
    SynchronizationDisabled,
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

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct CustomRelay {
    pub url: String,
    pub credential_configured: bool,
}

impl std::fmt::Debug for CustomRelay {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CustomRelay")
            .field("credential_configured", &self.credential_configured)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CustomRelayMutationRejection {
    InvalidUrl,
    Duplicate,
    NotFound,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct CustomRelayMutationResult {
    pub relays: Vec<CustomRelay>,
    pub rejection: Option<CustomRelayMutationRejection>,
}

impl std::fmt::Debug for CustomRelayMutationResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CustomRelayMutationResult")
            .field("relay_count", &self.relays.len())
            .field("rejection", &self.rejection)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectivityOpportunity {
    Foreground,
    SystemWake,
    NetworkChanged,
}

impl From<ConnectivityOpportunity> for uc_engine::ConnectivityOpportunity {
    fn from(reason: ConnectivityOpportunity) -> Self {
        match reason {
            ConnectivityOpportunity::Foreground => Self::Foreground,
            ConnectivityOpportunity::SystemWake => Self::SystemWake,
            ConnectivityOpportunity::NetworkChanged => Self::NetworkChanged,
        }
    }
}

enum WorkerCommand {
    NotifyConnectivityOpportunity {
        reason: ConnectivityOpportunity,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
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
    QueryCustomRelays {
        response: mpsc::Sender<Result<Vec<CustomRelay>, BindingError>>,
    },
    MutateCustomRelay {
        mutation: CustomRelayMutation,
        response: mpsc::Sender<Result<CustomRelayMutationResult, BindingError>>,
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
    QueryDeviceGroupChoices {
        response: mpsc::Sender<Result<String, BindingError>>,
    },
    ChooseDeviceGroup {
        issue_id: String,
        choice_id: String,
        expected_revision: u64,
        confirm_local_removal: bool,
        response: mpsc::Sender<Result<String, BindingError>>,
    },
    RemoveMember {
        device_id: String,
        response: mpsc::Sender<Result<WorkspaceConvergence, BindingError>>,
    },
    ResendEntry {
        entry_id: String,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<ResendEntryOutcome, BindingError>>,
    },
    LeaveSpace {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    CreateSpace {
        device_name: Option<String>,
        passphrase: Zeroizing<String>,
        response: mpsc::Sender<Result<SpaceCreated, BindingError>>,
    },
    IssueInvitation {
        response: mpsc::Sender<Result<InvitationIssued, BindingError>>,
    },
    ChangeEncryptionPassphrase {
        passphrase: Zeroizing<String>,
        passphrase_confirmation: Zeroizing<String>,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    JoinSpace {
        invitation_code: Zeroizing<String>,
        device_name: Option<String>,
        passphrase: Zeroizing<String>,
        preserve_unreadable_history: bool,
        response: mpsc::Sender<Result<JoinSpaceStatus, BindingError>>,
    },
    CancelJoinSpace {
        join_id: String,
        response: mpsc::Sender<Result<JoinSpaceStatus, BindingError>>,
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
}

#[derive(uniffi::Object)]
pub struct MobileEngine {
    commands: Mutex<Option<tokio::sync::mpsc::UnboundedSender<WorkerCommand>>>,
    lifecycle_commands: Mutex<Option<tokio::sync::mpsc::UnboundedSender<LifecycleCommand>>>,
    shutdown_pending: AtomicBool,
    events: Arc<EventQueue>,
    worker: WorkerJoin,
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
        // 宿主输入校验：无法解析的输入按固定错误码拒绝，拒绝原因已完整表达。
        let previous_distinct_id = change.previous_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        // 宿主输入校验：无法解析的输入按固定错误码拒绝，拒绝原因已完整表达。
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
        // 宿主输入校验：无法解析的输入按固定错误码拒绝，拒绝原因已完整表达。
        let previous_distinct_id = change.previous_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        // 宿主输入校验：无法解析的输入按固定错误码拒绝，拒绝原因已完整表达。
        let new_distinct_id = change.new_distinct_id.parse().map_err(|_| {
            Self::map_identity_error(crate::BindingAnalyticsHostError::InvalidIdentity)
        })?;
        Ok(ReleaseOutcome {
            previous_distinct_id,
            new_distinct_id,
        })
    }

    fn warn_callback(scope: &'static str, error: &crate::BindingAnalyticsHostError) {
        // 宿主回调错误是无字段的固定枚举，变体名即完整分类。
        tracing::warn!(scope, error_kind = ?error, "mobile analytics callback failed");
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
        startup_lifecycle: Option<&MobileStartupLifecycle>,
    ) -> Result<Arc<Self>, BindingError> {
        let capabilities = host_capabilities(Arc::clone(&host), analytics)?;
        let startup_lifecycle = startup_lifecycle
            .map(MobileStartupLifecycle::take_input)
            .transpose()?;
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
                    startup_lifecycle,
                )
            })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;

        match start_result.recv() {
            Ok(Ok(())) => Ok(Arc::new(Self {
                commands: Mutex::new(Some(commands)),
                lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
                shutdown_pending: AtomicBool::new(false),
                events,
                worker: WorkerJoin::new(worker),
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
        Self::start_inner(config, host, None, None)
    }

    #[uniffi::constructor]
    pub fn start_with_analytics(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
        analytics: Arc<dyn BindingAnalyticsHost>,
        context: BindingAnalyticsContext,
    ) -> Result<Arc<Self>, BindingError> {
        Self::start_inner(config, host, Some((analytics, context)), None)
    }

    #[uniffi::constructor]
    pub fn start_with_lifecycle(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
        lifecycle: Arc<MobileStartupLifecycle>,
    ) -> Result<Arc<Self>, BindingError> {
        Self::start_inner(config, host, None, Some(&lifecycle))
    }

    #[uniffi::constructor]
    pub fn start_with_analytics_and_lifecycle(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
        analytics: Arc<dyn BindingAnalyticsHost>,
        context: BindingAnalyticsContext,
        lifecycle: Arc<MobileStartupLifecycle>,
    ) -> Result<Arc<Self>, BindingError> {
        Self::start_inner(config, host, Some((analytics, context)), Some(&lifecycle))
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn query_local_device(&self) -> Result<LocalDevice, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::QueryLocalDevice { response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn notify_connectivity_opportunity(
        &self,
        reason: ConnectivityOpportunity,
    ) -> Result<(), BindingError> {
        self.request(|response| WorkerCommand::NotifyConnectivityOpportunity { reason, response })
    }

    pub fn refresh_peer_connections(&self) -> Result<PeerConnectionRefresh, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RefreshPeerConnections { response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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

    pub fn query_custom_relays(&self) -> Result<Vec<CustomRelay>, BindingError> {
        self.request(|response| WorkerCommand::QueryCustomRelays { response })
    }

    pub fn add_custom_relay(
        &self,
        url: String,
        access_token: String,
    ) -> Result<CustomRelayMutationResult, BindingError> {
        let mutation = CustomRelayMutation::Add {
            url,
            access_token: relay_token(access_token),
        };
        self.request(|response| WorkerCommand::MutateCustomRelay { mutation, response })
    }

    pub fn edit_custom_relay(
        &self,
        previous_url: String,
        url: String,
        access_token: String,
    ) -> Result<CustomRelayMutationResult, BindingError> {
        let mutation = CustomRelayMutation::Edit {
            previous_url,
            url,
            access_token: relay_token(access_token),
        };
        self.request(|response| WorkerCommand::MutateCustomRelay { mutation, response })
    }

    pub fn delete_custom_relay(
        &self,
        url: String,
    ) -> Result<CustomRelayMutationResult, BindingError> {
        let mutation = CustomRelayMutation::Delete { url };
        self.request(|response| WorkerCommand::MutateCustomRelay { mutation, response })
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

    pub fn query_device_group_choices(&self) -> Result<String, BindingError> {
        self.request(|response| WorkerCommand::QueryDeviceGroupChoices { response })
    }

    pub fn choose_device_group(
        &self,
        issue_id: String,
        choice_id: String,
        expected_revision: u64,
        confirm_local_removal: bool,
    ) -> Result<String, BindingError> {
        self.request(|response| WorkerCommand::ChooseDeviceGroup {
            issue_id,
            choice_id,
            expected_revision,
            confirm_local_removal,
            response,
        })
    }

    pub fn remove_member(&self, device_id: String) -> Result<WorkspaceConvergence, BindingError> {
        self.request(|response| WorkerCommand::RemoveMember {
            device_id,
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
            .send(LifecycleCommand::LifecycleState { response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn issue_invitation(&self) -> Result<InvitationIssued, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::IssueInvitation { response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn change_encryption_passphrase(
        &self,
        passphrase: String,
        passphrase_confirmation: String,
    ) -> Result<(), BindingError> {
        let passphrase = Zeroizing::new(passphrase);
        let passphrase_confirmation = Zeroizing::new(passphrase_confirmation);
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::ChangeEncryptionPassphrase {
                passphrase,
                passphrase_confirmation,
                response,
            })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn join_space(
        &self,
        invitation_code: String,
        device_name: Option<String>,
        passphrase: String,
        preserve_unreadable_history: bool,
    ) -> Result<JoinSpaceStatus, BindingError> {
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn cancel_join_space(&self, join_id: String) -> Result<JoinSpaceStatus, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::CancelJoinSpace { join_id, response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn capture_current_clipboard(&self) -> Result<Option<String>, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::CaptureCurrentClipboard { response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn shutdown(&self, deadline_ms: u64) -> Result<(), BindingError> {
        self.shutdown_inner(Duration::from_millis(deadline_ms), true)
    }

    pub fn suspend(&self) -> Result<(), BindingError> {
        self.suspend_inner(LIFECYCLE_TRANSITION_DEADLINE)
    }

    pub fn suspend_with_deadline(&self, deadline_ms: u64) -> Result<(), BindingError> {
        self.suspend_inner(Duration::from_millis(deadline_ms))
    }

    pub fn resume(&self) -> Result<(), BindingError> {
        let commands = self.lifecycle_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(LifecycleCommand::Resume { response })
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
    ) -> Result<tokio::sync::mpsc::UnboundedSender<LifecycleCommand>, BindingError> {
        lock(&self.lifecycle_commands)
            .as_ref()
            .cloned()
            .ok_or(BindingError::AlreadyStopped)
    }
}

impl Drop for MobileEngine {
    fn drop(&mut self) {
        if self.shutdown_inner(Duration::from_secs(5), true).is_err() {
            let _ = self.join_worker(Duration::ZERO);
        }
    }
}

fn run_worker(
    config: EngineConfig,
    host: HostCapabilities,
    requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    lifecycle_requests: tokio::sync::mpsc::UnboundedReceiver<LifecycleCommand>,
    events: Arc<EventQueue>,
    started: mpsc::Sender<Result<(), BindingError>>,
    startup_lifecycle: Option<StartupLifecycleInput>,
) -> Result<(), BindingError> {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = started.send(Err(BindingError::RuntimeUnavailable));
            return Err(BindingError::RuntimeUnavailable);
        }
    };
    runtime.block_on(run_worker_loop(
        config,
        host,
        requests,
        lifecycle_requests,
        events,
        started,
        startup_lifecycle,
    ))
}

async fn run_worker_loop(
    config: EngineConfig,
    host: HostCapabilities,
    requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    lifecycle_requests: tokio::sync::mpsc::UnboundedReceiver<LifecycleCommand>,
    events: Arc<EventQueue>,
    started: mpsc::Sender<Result<(), BindingError>>,
    startup_lifecycle: Option<StartupLifecycleInput>,
) -> Result<(), BindingError> {
    let result = match startup_lifecycle {
        Some(lifecycle) => {
            let (progress, _) = StartupProgress::channel();
            Engine::start_with_lifecycle(config, host, progress, lifecycle).await
        }
        None => Engine::start(config, host).await,
    };
    let (engine, mut engine_events) = match result {
        Ok(started_engine) => started_engine,
        Err(error) => {
            let error = BindingError::from(error);
            let _ = started.send(Err(error.clone()));
            return Err(error);
        }
    };
    let engine = Arc::new(engine);

    let forwarded_events = Arc::clone(&events);
    let event_task = tokio::spawn(async move {
        while let Some(event) = engine_events.next().await {
            forwarded_events.push(map_engine_event(event));
        }
        forwarded_events.close();
    });
    if started.send(Ok(())).is_err() {
        return worker_shutdown::finish_shutdown(
            engine.shutdown_until_complete(),
            event_task,
            events,
        )
        .await;
    }

    let shutdown_response = run_until_lifecycle_finishes(
        worker_lifecycle::run(Arc::clone(&engine), lifecycle_requests),
        run_operations(&engine, requests),
    )
    .await;
    let result =
        worker_shutdown::finish_shutdown(engine.shutdown_until_complete(), event_task, events)
            .await;
    for response in shutdown_response {
        let _ = response.send(result.clone());
    }
    result
}

async fn run_until_lifecycle_finishes<T>(
    lifecycle: impl Future<Output = T>,
    operations: impl Future<Output = ()>,
) -> T {
    tokio::pin!(lifecycle);
    tokio::pin!(operations);
    tokio::select! {
        biased;
        response = &mut lifecycle => response,
        () = &mut operations => lifecycle.await,
    }
}

async fn run_operations(
    engine: &Arc<Engine>,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
) {
    while let Some(command) = requests.recv().await {
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
                let result = engine
                    .execute(Operation::RecoverSession(RecoverSessionInput {
                        allow_secure_storage_unlock,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_session_recovery);
                let _ = response.send(result);
            }
            WorkerCommand::QueryLocalDevice { response } => {
                let result = engine
                    .execute(Operation::QueryLocalDevice)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_local_device);
                let _ = response.send(result);
            }
            WorkerCommand::NotifyConnectivityOpportunity { reason, response } => {
                let result = engine
                    .execute(Operation::NotifyConnectivityOpportunity {
                        reason: reason.into(),
                    })
                    .await
                    .map_err(BindingError::from)
                    .and_then(|result| match result {
                        OperationResult::ConnectivityOpportunityAccepted => Ok(()),
                        _ => Err(BindingError::UnexpectedResult),
                    });
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
                let (mutation, credential_url) =
                    match previous_url.filter(|previous| !previous.is_empty()) {
                        Some(previous_url) if url.is_empty() => {
                            (CustomRelayMutation::Delete { url: previous_url }, None)
                        }
                        Some(previous_url) => (
                            CustomRelayMutation::Edit {
                                previous_url,
                                url: url.clone(),
                                access_token: relay_token(access_token.to_string()),
                            },
                            Some(url),
                        ),
                        None => (
                            CustomRelayMutation::Add {
                                url: url.clone(),
                                access_token: relay_token(access_token.to_string()),
                            },
                            Some(url),
                        ),
                    };
                let result = map_legacy_relay_save(
                    &engine,
                    engine
                        .execute(Operation::MutateCustomRelay(mutation))
                        .await
                        .map_err(BindingError::from),
                    credential_url,
                )
                .await;
                let _ = response.send(result);
            }
            WorkerCommand::QueryCustomRelays { response } => {
                let result = engine
                    .execute(Operation::QueryCustomRelays)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_custom_relays);
                let _ = response.send(result);
            }
            WorkerCommand::MutateCustomRelay { mutation, response } => {
                let result = engine
                    .execute(Operation::MutateCustomRelay(mutation))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_custom_relay_mutation);
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
            WorkerCommand::QueryDeviceGroupChoices { response } => {
                let result = engine
                    .execute(Operation::QueryDeviceGroupChoices)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_device_group_choices);
                let _ = response.send(result);
            }
            WorkerCommand::ChooseDeviceGroup {
                issue_id,
                choice_id,
                expected_revision,
                confirm_local_removal,
                response,
            } => {
                let result = engine
                    .execute(Operation::ChooseDeviceGroup(
                        uc_engine::ChooseDeviceGroupInput {
                            issue_id,
                            choice_id,
                            expected_revision,
                            confirm_local_removal,
                        },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_device_group_choice_result);
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
                    .and_then(map_workspace_convergence);
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
            WorkerCommand::ChangeEncryptionPassphrase {
                passphrase,
                passphrase_confirmation,
                response,
            } => {
                let result = engine
                    .execute(Operation::ChangeEncryptionPassphrase(
                        ChangeEncryptionPassphraseInput {
                            passphrase: SecretString::new(passphrase.as_str()),
                            passphrase_confirmation: SecretString::new(
                                passphrase_confirmation.as_str(),
                            ),
                        },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_encryption_passphrase_changed);
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
                    .and_then(map_join_space_status);
                let _ = response.send(result);
            }
            WorkerCommand::CancelJoinSpace { join_id, response } => {
                let result = engine
                    .execute(Operation::CancelJoinSpace(
                        uc_engine::CancelJoinSpaceInput { join_id },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_join_space_status);
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
        }
    }
    // 关闭普通通道是关闭请求的第一步，生命周期通道仍负责最后确认。
    pending::<()>().await;
}

fn receive_lifecycle_result<T>(
    result: mpsc::Receiver<T>,
    deadline: Duration,
) -> Result<T, BindingError> {
    result
        .recv_timeout(deadline)
        // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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
        uc_engine::EngineEvent::DeviceTrustChanged { revision } => {
            BindingEvent::DeviceTrustChanged { revision }
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
        uc_engine::EngineEvent::RePairingRequired { scope } => BindingEvent::RePairingRequired {
            scope: match scope {
                uc_engine::RePairingScope::AllDevices => BindingRePairingScope::AllDevices,
            },
        },
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

map_enum!(map_workspace_convergence_phase, uc_engine::WorkspaceConvergencePhaseSummary => WorkspaceConvergencePhase,
    LocallyApplied, Converging, Complete, RecoveryRequired,
);

map_enum!(map_workspace_convergence_failure_category, uc_engine::WorkspaceConvergenceFailureCategorySummary => WorkspaceConvergenceFailureCategory,
    SpaceMismatch, ContinuityGap, IdentityMismatch, DigestConflict, Unauthorized, VersionIncompatible, NoEffectiveMembers, Storage,
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
        re_pairing_required: state.re_pairing_required,
        space_id: state.space_id,
        current_invitation: state.current_invitation.map(|invitation| SpaceInvitation {
            invitation_code: invitation.invitation_code,
            full_invitation: invitation.full_invitation,
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

fn map_workspace_convergence(
    result: OperationResult,
) -> Result<WorkspaceConvergence, BindingError> {
    unpack_operation!(result, OperationResult::WorkspaceMembership(summary) => {
        map_workspace_convergence_summary(summary)
    })
}

fn map_workspace_convergence_summary(
    summary: uc_engine::WorkspaceConvergenceSummary,
) -> WorkspaceConvergence {
    WorkspaceConvergence {
        phase: map_workspace_convergence_phase(summary.phase),
        revision: summary.revision,
        history_event_count: summary.history_event_count,
        effective_member_count: summary.effective_member_count,
        pending_removal_decision_device_ids: summary.pending_removal_decision_device_ids,
        pending_removal_decision_event_id: summary.pending_removal_decision_event_id,
        diverged_peer_device_ids: summary.diverged_peer_device_ids,
        upgrade_required_peer_device_ids: summary.upgrade_required_peer_device_ids,
        convergence_digest: summary.convergence_digest,
        removed: summary.removed,
        updated_at_ms: summary.updated_at_ms,
        failure_category: summary
            .failure_category
            .map(map_workspace_convergence_failure_category),
    }
}

fn map_device_group_choices(result: OperationResult) -> Result<String, BindingError> {
    match result {
        OperationResult::DeviceGroupChoices(summary) => {
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            serde_json::to_string(&summary).map_err(|_| BindingError::UnexpectedResult)
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_device_group_choice_result(result: OperationResult) -> Result<String, BindingError> {
    match result {
        OperationResult::DeviceGroupChosen(summary) => {
            // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
            serde_json::to_string(&summary).map_err(|_| BindingError::UnexpectedResult)
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

#[cfg(test)]
fn map_device_trust_snapshot(
    snapshot: uc_engine::DeviceTrustSnapshotSummary,
) -> Result<String, BindingError> {
    // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
    serde_json::to_string(&snapshot).map_err(|_| BindingError::UnexpectedResult)
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
        uc_engine::ResendEntryOutcome::SynchronizationDisabled => {
            ResendEntryOutcome::SynchronizationDisabled
        }
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
        full_invitation,
        expires_at_ms,
        availability,
    } => InvitationIssued {
        invitation_code,
        full_invitation,
        expires_at_ms,
        availability: map_invitation_availability(availability),
    })
}

fn map_encryption_passphrase_changed(result: OperationResult) -> Result<(), BindingError> {
    unpack_operation!(result, OperationResult::EncryptionPassphraseChanged => ())
}

fn map_join_space_status(result: OperationResult) -> Result<JoinSpaceStatus, BindingError> {
    let OperationResult::JoinSpace(status) = result else {
        return Err(BindingError::UnexpectedResult);
    };
    Ok(match status {
        uc_engine::JoinSpaceStatusSummary::Active {
            join_id,
            joined_space,
            peer_upgrade_required,
        } => JoinSpaceStatus::Active {
            join_id,
            joined_space: JoinedSpace {
                sponsor_device_id: joined_space.sponsor_device_id,
                sponsor_identity_fingerprint: joined_space.sponsor_identity_fingerprint,
                space_id: joined_space.space_id,
                self_device_id: joined_space.self_device_id,
                self_identity_fingerprint: joined_space.self_identity_fingerprint,
                migrated_records: joined_space.migrated_records,
                preserved_unreadable_records: joined_space.preserved_unreadable_records,
            },
            peer_upgrade_required,
        },
        uc_engine::JoinSpaceStatusSummary::Pending {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            cancel_requested,
            peer_upgrade_required,
        } => JoinSpaceStatus::Pending {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            cancel_requested,
            peer_upgrade_required,
        },
        uc_engine::JoinSpaceStatusSummary::Processing {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            peer_upgrade_required,
        } => JoinSpaceStatus::Processing {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            peer_upgrade_required,
        },
        uc_engine::JoinSpaceStatusSummary::NeedsAttention {
            join_id,
            reason,
            recovery,
            next_retry_at_ms,
        } => JoinSpaceStatus::NeedsAttention {
            join_id,
            reason: match reason {
                uc_engine::JoinSpaceAttentionReasonSummary::OutcomeCannotBeProven => {
                    JoinSpaceAttentionReason::OutcomeCannotBeProven
                }
            },
            recovery: match recovery {
                uc_engine::JoinSpaceAttentionRecoverySummary::PreserveDataAndContactSupport => {
                    JoinSpaceAttentionRecovery::PreserveDataAndContactSupport
                }
            },
            next_retry_at_ms,
        },
        uc_engine::JoinSpaceStatusSummary::Rejected { join_id, reason } => {
            JoinSpaceStatus::Rejected {
                join_id,
                reason: match reason {
                    uc_engine::JoinSpaceRejectionReasonSummary::InvitationUnavailable => {
                        JoinSpaceRejectionReason::InvitationUnavailable
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::AuthenticationRejected => {
                        JoinSpaceRejectionReason::AuthenticationRejected
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::IdentityConflict => {
                        JoinSpaceRejectionReason::IdentityConflict
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::BaseHistoryChanged => {
                        JoinSpaceRejectionReason::BaseHistoryChanged
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::JoinerHistoryAhead => {
                        JoinSpaceRejectionReason::JoinerHistoryAhead
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::HistoryConflict => {
                        JoinSpaceRejectionReason::HistoryConflict
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::CompletionInvalid => {
                        JoinSpaceRejectionReason::CompletionInvalid
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::MembershipHistoryInvalid => {
                        JoinSpaceRejectionReason::MembershipHistoryInvalid
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::SecurityMaterialInvalid => {
                        JoinSpaceRejectionReason::SecurityMaterialInvalid
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::RelationshipConflict => {
                        JoinSpaceRejectionReason::RelationshipConflict
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::ActivationStateInvalid => {
                        JoinSpaceRejectionReason::ActivationStateInvalid
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::PeerUpgradeRequired => {
                        JoinSpaceRejectionReason::PeerUpgradeRequired
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::Cancelled => {
                        JoinSpaceRejectionReason::Cancelled
                    }
                    uc_engine::JoinSpaceRejectionReasonSummary::RemovedBeforeActivation => {
                        JoinSpaceRejectionReason::RemovedBeforeActivation
                    }
                },
            }
        }
        uc_engine::JoinSpaceStatusSummary::Terminated { join_id, reason } => {
            JoinSpaceStatus::Terminated {
                join_id,
                reason: match reason {
                    uc_engine::JoinSpaceTerminationReasonSummary::Cancelled => {
                        JoinSpaceTerminationReason::Cancelled
                    }
                    uc_engine::JoinSpaceTerminationReasonSummary::Expired => {
                        JoinSpaceTerminationReason::Expired
                    }
                    uc_engine::JoinSpaceTerminationReasonSummary::Superseded => {
                        JoinSpaceTerminationReason::Superseded
                    }
                },
            }
        }
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

fn relay_token(value: String) -> Option<SecretString> {
    (!value.is_empty()).then(|| SecretString::new(value))
}

fn map_custom_relays(result: OperationResult) -> Result<Vec<CustomRelay>, BindingError> {
    match result {
        OperationResult::CustomRelays(relays) => Ok(relays
            .into_iter()
            .map(|relay| CustomRelay {
                url: relay.url,
                credential_configured: relay.credential_configured,
            })
            .collect()),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_custom_relay_mutation(
    result: OperationResult,
) -> Result<CustomRelayMutationResult, BindingError> {
    match result {
        OperationResult::CustomRelayMutated(CustomRelayMutationOutcome::Saved { relays }) => {
            Ok(CustomRelayMutationResult {
                relays: relays
                    .into_iter()
                    .map(|relay| CustomRelay {
                        url: relay.url,
                        credential_configured: relay.credential_configured,
                    })
                    .collect(),
                rejection: None,
            })
        }
        OperationResult::CustomRelayMutated(CustomRelayMutationOutcome::Rejected { reason }) => {
            Ok(CustomRelayMutationResult {
                relays: Vec::new(),
                rejection: Some(match reason {
                    CustomRelayRejection::InvalidUrl => CustomRelayMutationRejection::InvalidUrl,
                    CustomRelayRejection::Duplicate => CustomRelayMutationRejection::Duplicate,
                    CustomRelayRejection::NotFound => CustomRelayMutationRejection::NotFound,
                }),
            })
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

async fn map_legacy_relay_save(
    engine: &Engine,
    result: Result<OperationResult, BindingError>,
    credential_url: Option<String>,
) -> Result<RelaySaveResult, BindingError> {
    let result = result?;
    let configured = match (&result, credential_url) {
        (
            OperationResult::CustomRelayMutated(CustomRelayMutationOutcome::Saved { .. }),
            Some(url),
        ) => match engine
            .execute(Operation::QueryRelayCredential(
                uc_engine::RelayCredentialInput { url },
            ))
            .await
            .map_err(BindingError::from)?
        {
            OperationResult::RelayCredentialStatus(status) => status.configured,
            _ => return Err(BindingError::UnexpectedResult),
        },
        _ => false,
    };
    finish_legacy_relay_save(result, configured)
}

fn finish_legacy_relay_save(
    result: OperationResult,
    configured: bool,
) -> Result<RelaySaveResult, BindingError> {
    match result {
        OperationResult::CustomRelayMutated(CustomRelayMutationOutcome::Saved { .. }) => {
            Ok(RelaySaveResult { configured })
        }
        OperationResult::CustomRelayMutated(CustomRelayMutationOutcome::Rejected { .. }) => {
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
    // TryFromIntError：目标分类完整表达数值范围不符。
    u64::try_from(value).map_err(|_| BindingError::UnexpectedResult)
}

pub(crate) fn host_directories(
    host: &Arc<dyn BindingHost>,
) -> Result<HostDirectories, BindingError> {
    let cache_directory = host_path(host.cache_directory())?;
    Ok(HostDirectories::new(
        host_path(host.private_data_directory())?,
        cache_directory.clone(),
        host_path(host.temporary_directory())?,
        cache_directory.join("logs"),
    ))
}

fn host_capabilities(
    host: Arc<dyn BindingHost>,
    analytics: Option<(Arc<dyn BindingAnalyticsHost>, BindingAnalyticsContext)>,
) -> Result<HostCapabilities, BindingError> {
    let directories = host_directories(&host)?;
    for directory in [
        directories.private_data(),
        directories.cache(),
        directories.temporary(),
    ] {
        // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
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

    #[tokio::test]
    async fn ordinary_requests_ending_does_not_drop_a_slow_lifecycle_reply() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let lifecycle_started = Arc::clone(&started);
        let lifecycle_release = Arc::clone(&release);
        let result = tokio::spawn(run_until_lifecycle_finishes(
            async move {
                lifecycle_started.notify_one();
                lifecycle_release.notified().await;
                42
            },
            async {},
        ));

        started.notified().await;
        tokio::task::yield_now().await;
        assert!(!result.is_finished());
        release.notify_one();
        assert_eq!(result.await.unwrap(), 42);
    }

    #[test]
    fn device_trust_json_keeps_complete_snapshot_fields() {
        let mut snapshot =
            uc_engine::DeviceTrustSnapshotSummary::empty_unavailable("local-device".into());
        snapshot
            .devices
            .push(uc_engine::DeviceTrustRelationshipSummary {
                device_id: "peer-device".into(),
                display_name: "Peer Device".into(),
                is_local: false,
                reachability: uc_engine::DeviceReachabilitySummary::Offline,
                membership: uc_engine::DeviceMembershipSummary::Active,
                group_relationship: uc_engine::DeviceGroupRelationshipSummary::Consistent,
                compatibility: uc_engine::DeviceCompatibilitySummary::Compatible,
                sync_relationship: uc_engine::DeviceSyncRelationshipSummary::Usable,
                pairing_confirmation: Some(
                    uc_engine::PairingConfirmationSummary::AwaitingPeerConfirmation,
                ),
                available_actions: Vec::new(),
                blocked_reason: None,
            });
        for (status, expected) in [
            (
                uc_engine::PairingConfirmationSummary::AwaitingPeerConfirmation,
                "awaiting_peer_confirmation",
            ),
            (
                uc_engine::PairingConfirmationSummary::Unconfirmed,
                "unconfirmed",
            ),
            (
                uc_engine::PairingConfirmationSummary::Confirmed,
                "confirmed",
            ),
        ] {
            snapshot.devices[0].pairing_confirmation = Some(status);
            let json = map_device_trust_snapshot(snapshot.clone()).unwrap();
            assert!(json.contains("local_device_id"));
            assert!(json.contains("current_change"));
            assert!(json.contains("devices"));
            assert!(json.contains("recovery"));
            assert!(json.contains("allowed_actions"));
            assert!(json.contains("blocked_reason"));
            assert!(json.contains(&format!("\"pairing_confirmation\":\"{expected}\"")));
        }
    }

    #[test]
    fn legacy_relay_save_does_not_report_another_relays_credential() {
        let relays = vec![
            uc_engine::CustomRelaySummary {
                url: "https://other-relay.example/".to_string(),
                credential_configured: true,
            },
            uc_engine::CustomRelaySummary {
                url: "https://target-relay.example/".to_string(),
                credential_configured: false,
            },
        ];
        let result = finish_legacy_relay_save(
            OperationResult::CustomRelayMutated(CustomRelayMutationOutcome::Saved { relays }),
            false,
        )
        .expect("legacy relay save must map");

        assert!(!result.configured);
    }

    #[test]
    fn join_space_mapping_preserves_history_counts_and_upgrade_prompt() {
        let joined = map_join_space_status(OperationResult::JoinSpace(
            uc_engine::JoinSpaceStatusSummary::Active {
                join_id: "join-id".into(),
                joined_space: uc_engine::JoinedSpaceSummary {
                    sponsor_device_id: "sponsor".into(),
                    sponsor_identity_fingerprint: "sponsor-fingerprint".into(),
                    space_id: "space".into(),
                    self_device_id: "self".into(),
                    self_identity_fingerprint: "self-fingerprint".into(),
                    migrated_records: Some(4),
                    preserved_unreadable_records: Some(2),
                },
                peer_upgrade_required: true,
            },
        ))
        .expect("join-space result must map");

        assert!(matches!(
            joined,
            JoinSpaceStatus::Active {
                joined_space,
                peer_upgrade_required: true,
                ..
            }
                if joined_space.migrated_records == Some(4)
                    && joined_space.preserved_unreadable_records == Some(2)
        ));
    }

    #[test]
    fn join_space_mapping_preserves_local_termination() {
        let status = map_join_space_status(OperationResult::JoinSpace(
            uc_engine::JoinSpaceStatusSummary::Terminated {
                join_id: "join-id".into(),
                reason: uc_engine::JoinSpaceTerminationReasonSummary::Expired,
            },
        ))
        .expect("join-space result must map");

        assert_eq!(
            status,
            JoinSpaceStatus::Terminated {
                join_id: "join-id".into(),
                reason: JoinSpaceTerminationReason::Expired,
            }
        );
    }

    #[test]
    fn join_space_mapping_preserves_processing_state() {
        let status = map_join_space_status(OperationResult::JoinSpace(
            uc_engine::JoinSpaceStatusSummary::Processing {
                join_id: "join-id".into(),
                target_space_id: "space-id".into(),
                sponsor_device_id: "sponsor-id".into(),
                sponsor_identity_fingerprint: "sponsor-fingerprint".into(),
                peer_upgrade_required: false,
            },
        ))
        .expect("processing join-space result must map");

        assert_eq!(
            status,
            JoinSpaceStatus::Processing {
                join_id: "join-id".into(),
                target_space_id: "space-id".into(),
                sponsor_device_id: "sponsor-id".into(),
                sponsor_identity_fingerprint: "sponsor-fingerprint".into(),
                peer_upgrade_required: false,
            }
        );
    }

    #[test]
    fn join_space_mapping_preserves_attention_recovery_contract() {
        let status = map_join_space_status(OperationResult::JoinSpace(
            uc_engine::JoinSpaceStatusSummary::NeedsAttention {
                join_id: "join-id".into(),
                reason: uc_engine::JoinSpaceAttentionReasonSummary::OutcomeCannotBeProven,
                recovery:
                    uc_engine::JoinSpaceAttentionRecoverySummary::PreserveDataAndContactSupport,
                next_retry_at_ms: None,
            },
        ))
        .expect("attention join-space result must map");

        assert_eq!(
            status,
            JoinSpaceStatus::NeedsAttention {
                join_id: "join-id".into(),
                reason: JoinSpaceAttentionReason::OutcomeCannotBeProven,
                recovery: JoinSpaceAttentionRecovery::PreserveDataAndContactSupport,
                next_retry_at_ms: None,
            }
        );
    }

    #[test]
    fn shutdown_deadline_bounds_the_worker_reply_and_join() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(1));
        let worker = std::thread::spawn(move || {
            drop(requests);
            if let Some(LifecycleCommand::Shutdown { response, .. }) =
                lifecycle_requests.blocking_recv()
            {
                std::thread::sleep(Duration::from_millis(200));
                let _ = response.send(Ok(()));
            }
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            events,
            worker: WorkerJoin::new(worker),
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
                if let Some(LifecycleCommand::Suspend { response, .. }) =
                    lifecycle_requests.blocking_recv()
                {
                    let _ = response.send(Ok(()));
                }
                let _ = recovery.join();
            }
            Ok(())
        });
        let engine = Arc::new(MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            events,
            worker: WorkerJoin::new(worker),
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
                full_invitation: "ucspace1_NEVER-SHOW-FULL".to_owned(),
                expires_at_ms: 1,
                availability: engine_availability,
            })
            .expect("invitation result must map into the mobile binding");

            assert_eq!(invitation.availability, binding_availability);
        }
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
    fn re_pairing_event_keeps_the_affected_device_scope() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::RePairingRequired {
                scope: uc_engine::RePairingScope::AllDevices,
            }),
            BindingEvent::RePairingRequired {
                scope: BindingRePairingScope::AllDevices,
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
                    entry_id: Some("entry-1".to_owned()),
                    attempt_id: Some("attempt-1".to_owned()),
                    status: "completed".to_owned(),
                    reason: None,
                },
            )),
            BindingEvent::TransferStatusChanged {
                transfer_id: "transfer-1".to_owned(),
                entry_id: Some("entry-1".to_owned()),
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
