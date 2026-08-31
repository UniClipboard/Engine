use std::fmt;

use serde::{Deserialize, Serialize};

use super::{EngineError, ResendEntryOutcome, SendReportSummary};
use crate::{
    ConfigExportOutcome, ConfigImportPreviewOutcome, ConfigImportStageOutcome,
    DebugModeUpdateSummary, DiagnosticLogsExportSummary, DiagnosticsStatusSummary,
    MobileAuthenticatedSession, MobileAuthenticationOutcome, MobileDeviceRegistrationOutcome,
    MobileDeviceRevokeOutcome, MobileDeviceSummary, MobileDeviceUpdateOutcome,
    MobileFileUploadHandle, MobileLanInterfaceSummary, MobileSyncDocument,
    MobileSyncDocumentApplyOutcome, MobileSyncFileReadOutcome, MobileSyncSettingsSummary,
    MobileSyncSettingsUpdateOutcome, RelayCredentialStatus, RelayProbeOutcome, SaveRelayOutcome,
    SettingsSummary, SettingsUpdateOutcome, UpgradeStatusSummary,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationTerminal {
    Succeeded,
    Failed(EngineError),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRecoveryPhaseSummary {
    Idle,
    Recovering,
    RetryScheduled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRecoveryStatusSummary {
    pub phase: NetworkRecoveryPhaseSummary,
    pub retryable: bool,
    pub next_retry_in_ms: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrySummary {
    pub entry_id: String,
    pub content_type: String,
    pub preview: Option<String>,
    pub created_at_ms: i64,
}

impl fmt::Debug for EntrySummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntrySummary")
            .field("entry_id", &self.entry_id)
            .field("content_type", &self.content_type)
            .field("has_preview", &self.preview.is_some())
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntrySummary {
    pub entry_id: String,
    pub preview: String,
    pub has_detail: bool,
    pub size_bytes: i64,
    pub captured_at_ms: i64,
    pub content_type: String,
    pub thumbnail_url: Option<String>,
    pub is_encrypted: bool,
    pub is_favorited: bool,
    pub updated_at_ms: i64,
    pub active_time_ms: i64,
    pub file_transfer_status: Option<String>,
    pub file_transfer_reason: Option<String>,
    pub content_tags: Vec<String>,
    pub link_urls: Option<Vec<String>>,
    pub link_domains: Option<Vec<String>>,
    pub file_sizes: Option<Vec<i64>>,
    pub image_width: Option<i32>,
    pub image_height: Option<i32>,
    pub is_directory: bool,
    pub payload_state: Option<String>,
}

impl fmt::Debug for HistoryEntrySummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoryEntrySummary")
            .field("entry_id", &self.entry_id)
            .field("has_preview", &!self.preview.is_empty())
            .field("has_detail", &self.has_detail)
            .field("size_bytes", &self.size_bytes)
            .field("content_type", &self.content_type)
            .field("has_thumbnail", &self.thumbnail_url.is_some())
            .field("is_encrypted", &self.is_encrypted)
            .field("is_favorited", &self.is_favorited)
            .field("tag_count", &self.content_tags.len())
            .field("link_count", &self.link_urls.as_ref().map_or(0, Vec::len))
            .field("is_directory", &self.is_directory)
            .field("has_payload_state", &self.payload_state.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntryDetailSummary {
    pub entry_id: String,
    pub content: String,
    pub size_bytes: i64,
    pub created_at_ms: i64,
    pub active_time_ms: i64,
    pub mime_type: Option<String>,
}

impl fmt::Debug for HistoryEntryDetailSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoryEntryDetailSummary")
            .field("entry_id", &self.entry_id)
            .field("has_content", &!self.content.is_empty())
            .field("size_bytes", &self.size_bytes)
            .field("created_at_ms", &self.created_at_ms)
            .field("active_time_ms", &self.active_time_ms)
            .field("mime_type", &self.mime_type)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntryResourceSummary {
    pub blob_id: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    pub url: Option<String>,
    pub inline_data: Option<Vec<u8>>,
}

impl fmt::Debug for HistoryEntryResourceSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoryEntryResourceSummary")
            .field("has_blob", &self.blob_id.is_some())
            .field("mime_type", &self.mime_type)
            .field("size_bytes", &self.size_bytes)
            .field("has_url", &self.url.is_some())
            .field("inline_byte_len", &self.inline_data.as_ref().map(Vec::len))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryStatsSummary {
    pub total_items: i64,
    pub total_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryClearSummary {
    pub deleted_count: u64,
    pub failed_entry_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryDeliveryViewSummary {
    pub entry_id: String,
    pub source: EntrySourceSummary,
    pub deliveries: Vec<EntryDeliveryTargetSummary>,
}

impl fmt::Debug for EntryDeliveryViewSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryDeliveryViewSummary")
            .field("entry_id", &self.entry_id)
            .field("source", &self.source)
            .field("delivery_count", &self.deliveries.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EntrySourceSummary {
    Local,
    Remote {
        device_id: String,
        device_name: Option<String>,
    },
    Historical,
}

impl fmt::Debug for EntrySourceSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("EntrySourceSummary");
        match self {
            Self::Local => debug.field("kind", &"local"),
            Self::Remote {
                device_id,
                device_name,
            } => debug
                .field("kind", &"remote")
                .field("device_id", device_id)
                .field("has_device_name", &device_name.is_some()),
            Self::Historical => debug.field("kind", &"historical"),
        };
        debug.finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryDeliveryTargetSummary {
    pub target_device_id: String,
    pub target_device_name: Option<String>,
    pub status: EntryDeliveryStatusSummary,
    pub reason_detail: Option<String>,
    pub updated_at_ms: Option<i64>,
}

impl fmt::Debug for EntryDeliveryTargetSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryDeliveryTargetSummary")
            .field("target_device_id", &self.target_device_id)
            .field("has_target_device_name", &self.target_device_name.is_some())
            .field("status", &self.status)
            .field("has_reason_detail", &self.reason_detail.is_some())
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EntryDeliveryStatusSummary {
    Pending,
    Delivered,
    Duplicate,
    Unreachable,
    Superseded,
    Failed {
        reason: DeliveryFailureReasonSummary,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFailureReasonSummary {
    LocalPolicy,
    PeerRejected,
    PeerIncompatible,
    Io,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveProgressSummary {
    pub entry_id: String,
    pub attempt_id: String,
    pub state: String,
    pub total_bytes: i64,
    pub completed_bytes: i64,
    pub items_total: u32,
    pub items_completed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveReadinessSummary {
    pub ready: bool,
    pub degraded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryReceiveCancellationOutcome {
    Cancelled,
    NotReceiving,
    TooLate,
    AlreadyTerminal,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundTransferCancellationOutcome {
    Cancelled,
    NotInflight,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ClipboardRestoreOutcome {
    Restored,
    PayloadUnavailable {
        entry_id: String,
        representation_id: String,
        state: String,
    },
    NotApplicable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveClipboardSummary {
    pub entry_id: String,
    pub activated_by: String,
}

impl fmt::Debug for ClipboardRestoreOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ClipboardRestoreOutcome");
        match self {
            Self::Restored => debug.field("kind", &"restored"),
            Self::PayloadUnavailable { state, .. } => debug
                .field("kind", &"payload_unavailable")
                .field("state", state),
            Self::NotApplicable { .. } => debug.field("kind", &"not_applicable"),
        };
        debug.finish()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryResourceSummary {
    pub bytes: Vec<u8>,
    pub media_type: Option<String>,
}

impl fmt::Debug for BinaryResourceSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BinaryResourceSummary")
            .field("byte_len", &self.bytes.len())
            .field("has_media_type", &self.media_type.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryFileResourceSummary {
    pub bytes: Vec<u8>,
    pub media_type: Option<String>,
    pub file_name: String,
}

impl fmt::Debug for EntryFileResourceSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryFileResourceSummary")
            .field("byte_len", &self.bytes.len())
            .field("has_media_type", &self.media_type.is_some())
            .field("has_file_name", &!self.file_name.is_empty())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum InvitationAvailability {
    CrossNetwork,
    SameLocalNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinedSpaceSummary {
    pub sponsor_device_id: String,
    pub sponsor_identity_fingerprint: String,
    pub space_id: String,
    pub self_device_id: String,
    pub self_identity_fingerprint: String,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinSpaceRejectionReasonSummary {
    InvitationUnavailable,
    AuthenticationRejected,
    IdentityConflict,
    BaseHistoryChanged,
    JoinerHistoryAhead,
    HistoryConflict,
    PeerUpgradeRequired,
    Cancelled,
    RemovedBeforeActivation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum JoinSpaceStatusSummary {
    Active {
        join_id: String,
        joined_space: JoinedSpaceSummary,
    },
    Pending {
        join_id: String,
        target_space_id: Option<String>,
        sponsor_device_id: Option<String>,
        sponsor_identity_fingerprint: Option<String>,
        cancel_requested: bool,
    },
    Rejected {
        join_id: String,
        reason: JoinSpaceRejectionReasonSummary,
    },
}

impl fmt::Debug for InvitationAvailability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CrossNetwork => "cross_network",
            Self::SameLocalNetwork => "same_local_network",
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum OperationResult {
    SpaceCreated {
        space_id: String,
        self_device_id: String,
        identity_fingerprint: String,
    },
    JoinSpace(JoinSpaceStatusSummary),
    SpaceUnlocked {
        space_id: String,
    },
    SessionRecovered {
        unlocked: bool,
        resumed: bool,
    },
    InvitationIssued {
        invitation_code: String,
        full_invitation: String,
        expires_at_ms: i64,
        availability: InvitationAvailability,
    },
    InvitationCancelled,
    SpaceReset,
    SpaceFactoryReset,
    SetupState(SetupStateSummary),
    StorageStats(StorageStatsSummary),
    StorageCacheCleared {
        freed_bytes: u64,
    },
    LocalDevice(LocalDeviceSummary),
    PeerConnections(Vec<PeerConnectionSummary>),
    PeerConnectionsRefreshed(PeerConnectionRefreshSummary),
    NetworkRecovered,
    NetworkRecoveryStatus(NetworkRecoveryStatusSummary),
    Settings(Box<SettingsSummary>),
    SettingsUpdated(SettingsUpdateOutcome),
    RelaySaved(SaveRelayOutcome),
    RelayProbed(RelayProbeOutcome),
    RelayCredentialStatus(RelayCredentialStatus),
    UpgradeStatus(UpgradeStatusSummary),
    UpgradeAcknowledged {
        version: String,
    },
    DiagnosticsStatus(DiagnosticsStatusSummary),
    DebugModeUpdated(DebugModeUpdateSummary),
    DiagnosticLogsExported(DiagnosticLogsExportSummary),
    ConfigExport(ConfigExportOutcome),
    ConfigImportPreview(ConfigImportPreviewOutcome),
    ConfigImportStaged(ConfigImportStageOutcome),
    MobileDevices(Vec<MobileDeviceSummary>),
    MobileDeviceRevoked(MobileDeviceRevokeOutcome),
    MobileRequestAuthenticated(MobileAuthenticatedSession),
    MobileAuthentication(MobileAuthenticationOutcome),
    MobileCredentialCurrent {
        current: bool,
    },
    MobileLanInterfaces(Vec<MobileLanInterfaceSummary>),
    MobileSyncSettings(Box<MobileSyncSettingsSummary>),
    MobileSyncSettingsUpdated(MobileSyncSettingsUpdateOutcome),
    MobileLanEndpointUpdated,
    MobileDeviceRegistered(MobileDeviceRegistrationOutcome),
    MobileDeviceUpdated(MobileDeviceUpdateOutcome),
    MobileContentAvailability {
        available: bool,
    },
    MobileSyncDocument(Option<Box<MobileSyncDocument>>),
    MobileSyncDocumentApplied(MobileSyncDocumentApplyOutcome),
    MobileSyncFile(MobileSyncFileReadOutcome),
    MobileFileUploadStarted(MobileFileUploadHandle),
    MobileFileUploadChunkAppended,
    MobileFileUploadFinished(MobileSyncDocumentApplyOutcome),
    MobileFileUploadAborted {
        existed: bool,
    },
    ReceiveReadiness(ReceiveReadinessSummary),
    EncryptionState(EncryptionStateSummary),
    EncryptionLocked,
    SecureStorageAccess {
        granted: bool,
    },
    Devices(Vec<DeviceSummary>),
    MemberSyncPreferences(MemberSyncPreferencesSummary),
    WorkspaceMembership(WorkspaceConvergenceSummary),
    DeviceTrust(DeviceTrustSnapshotSummary),
    DeviceGroupChoices(DeviceGroupChoicesSummary),
    DeviceGroupChosen(DeviceGroupChoiceResultSummary),
    #[cfg(feature = "dev-tools")]
    MembershipDiagnostics(MembershipDiagnosticsSummary),
    SpaceProtection(SpaceProtectionSummary),
    SearchPage(SearchPageSummary),
    SearchTags(Vec<SearchTagSummary>),
    SearchStatus(SearchStatusSummary),
    SearchRebuildAccepted {
        accepted: bool,
    },
    EntrySent(SendReportSummary),
    HistoryPage {
        entries: Vec<EntrySummary>,
        next_cursor: Option<String>,
    },
    HistoryEntries(Vec<HistoryEntrySummary>),
    HistoryEntry(HistoryEntryDetailSummary),
    HistoryEntryDeleted,
    HistoryEntryFavoriteSet,
    HistoryStats(HistoryStatsSummary),
    HistoryEntryResource(HistoryEntryResourceSummary),
    BlobRead(BinaryResourceSummary),
    ThumbnailRead(BinaryResourceSummary),
    EntryFileRead(EntryFileResourceSummary),
    EntryDelivery(EntryDeliveryViewSummary),
    HistoryCleared(HistoryClearSummary),
    EntryReceiveProgress(Option<ReceiveProgressSummary>),
    EntryReceiveProgressList(Vec<ReceiveProgressSummary>),
    EntryReceiveCancellation(EntryReceiveCancellationOutcome),
    InboundTransferCancellation(InboundTransferCancellationOutcome),
    ClipboardCaptured {
        entry_id: Option<String>,
    },
    ClipboardChangeObserved {
        report: Option<SendReportSummary>,
    },
    ActiveClipboard(Option<ActiveClipboardSummary>),
    ClipboardRestored(ClipboardRestoreOutcome),
    EntryExported,
    EntryResent(ResendEntryOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoicesSummary {
    pub revision: u64,
    pub device_trust: DeviceTrustSnapshotSummary,
    pub issues: Vec<DeviceGroupChoiceIssueSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceIssueSummary {
    pub issue_id: String,
    pub choices: Vec<DeviceGroupChoiceOptionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceOptionSummary {
    pub choice_id: String,
    pub is_current_group: bool,
    pub requires_re_pairing: bool,
    pub member_device_ids: Vec<String>,
    pub members_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGroupChoiceOutcomeSummary {
    Completed,
    Pending,
    RePairingRequired,
    AlreadyCompleted,
    StateChanged,
    LocalDeviceConfirmationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroupChoiceResultSummary {
    pub outcome: DeviceGroupChoiceOutcomeSummary,
    pub current_revision: Option<u64>,
}

#[cfg(feature = "dev-tools")]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDiagnosticsSummary {
    pub revision: u64,
    pub branch_id: String,
    pub head_event_id: String,
    pub group_epoch: u64,
    pub effective_member_count: u32,
    pub pending_conflict_count: u32,
    pub pending_effect_count: u32,
    pub transition_phases: Vec<String>,
}

#[cfg(feature = "dev-tools")]
impl fmt::Debug for MembershipDiagnosticsSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MembershipDiagnosticsSummary")
            .field("identifiers", &"[REDACTED]")
            .field("revision", &self.revision)
            .field("group_epoch", &self.group_epoch)
            .field("effective_member_count", &self.effective_member_count)
            .field("pending_conflict_count", &self.pending_conflict_count)
            .field("pending_effect_count", &self.pending_effect_count)
            .field("transition_phases", &self.transition_phases)
            .finish()
    }
}

impl fmt::Debug for OperationResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("OperationResult");
        match self {
            Self::SpaceCreated { .. } => debug.field("kind", &"space_created"),
            Self::JoinSpace(status) => debug.field("kind", &"join_space").field("status", status),
            Self::SpaceUnlocked { .. } => debug.field("kind", &"space_unlocked"),
            Self::SessionRecovered { unlocked, resumed } => debug
                .field("kind", &"session_recovered")
                .field("unlocked", unlocked)
                .field("resumed", resumed),
            Self::InvitationIssued { .. } => debug.field("kind", &"invitation_issued"),
            Self::InvitationCancelled => debug.field("kind", &"invitation_cancelled"),
            Self::SpaceReset => debug.field("kind", &"space_reset"),
            Self::SpaceFactoryReset => debug.field("kind", &"space_factory_reset"),
            Self::SetupState(state) => debug.field("kind", &"setup_state").field("state", state),
            Self::StorageStats(stats) => {
                debug.field("kind", &"storage_stats").field("stats", stats)
            }
            Self::StorageCacheCleared { freed_bytes } => debug
                .field("kind", &"storage_cache_cleared")
                .field("freed_bytes", freed_bytes),
            Self::LocalDevice(device) => {
                debug.field("kind", &"local_device").field("device", device)
            }
            Self::PeerConnections(peers) => debug
                .field("kind", &"peer_connections")
                .field("peer_count", &peers.len()),
            Self::PeerConnectionsRefreshed(report) => debug
                .field("kind", &"peer_connections_refreshed")
                .field("report", report),
            Self::NetworkRecovered => debug.field("kind", &"network_recovered"),
            Self::NetworkRecoveryStatus(status) => debug
                .field("kind", &"network_recovery_status")
                .field("status", status),
            Self::Settings(_) => debug.field("kind", &"settings"),
            Self::SettingsUpdated(outcome) => debug
                .field("kind", &"settings_updated")
                .field("outcome", outcome),
            Self::RelaySaved(outcome) => debug
                .field("kind", &"relay_saved")
                .field("outcome", outcome),
            Self::RelayProbed(outcome) => debug
                .field("kind", &"relay_probed")
                .field("outcome", outcome),
            Self::RelayCredentialStatus(status) => debug
                .field("kind", &"relay_credential_status")
                .field("configured", &status.configured),
            Self::UpgradeStatus(status) => debug
                .field("kind", &"upgrade_status")
                .field("status", status),
            Self::UpgradeAcknowledged { version } => debug
                .field("kind", &"upgrade_acknowledged")
                .field("version", version),
            Self::DiagnosticsStatus(status) => debug
                .field("kind", &"diagnostics_status")
                .field("status", status),
            Self::DebugModeUpdated(result) => debug
                .field("kind", &"debug_mode_updated")
                .field("result", result),
            Self::DiagnosticLogsExported(result) => debug
                .field("kind", &"diagnostic_logs_exported")
                .field("result", result),
            Self::ConfigExport(outcome) => debug
                .field("kind", &"config_export")
                .field("outcome", outcome),
            Self::ConfigImportPreview(outcome) => debug
                .field("kind", &"config_import_preview")
                .field("outcome", outcome),
            Self::ConfigImportStaged(outcome) => debug
                .field("kind", &"config_import_staged")
                .field("outcome", outcome),
            Self::MobileDevices(devices) => debug
                .field("kind", &"mobile_devices")
                .field("device_count", &devices.len()),
            Self::MobileDeviceRevoked(outcome) => debug
                .field("kind", &"mobile_device_revoked")
                .field("outcome", outcome),
            Self::MobileRequestAuthenticated(session) => debug
                .field("kind", &"mobile_request_authenticated")
                .field("session", session),
            Self::MobileAuthentication(outcome) => debug
                .field("kind", &"mobile_authentication")
                .field("outcome", outcome),
            Self::MobileCredentialCurrent { current } => debug
                .field("kind", &"mobile_credential_current")
                .field("current", current),
            Self::MobileLanInterfaces(interfaces) => debug
                .field("kind", &"mobile_lan_interfaces")
                .field("interface_count", &interfaces.len()),
            Self::MobileSyncSettings(settings) => debug
                .field("kind", &"mobile_sync_settings")
                .field("settings", settings),
            Self::MobileSyncSettingsUpdated(outcome) => debug
                .field("kind", &"mobile_sync_settings_updated")
                .field("outcome", outcome),
            Self::MobileLanEndpointUpdated => debug.field("kind", &"mobile_lan_endpoint_updated"),
            Self::MobileDeviceRegistered(outcome) => debug
                .field("kind", &"mobile_device_registered")
                .field("outcome", outcome),
            Self::MobileDeviceUpdated(outcome) => debug
                .field("kind", &"mobile_device_updated")
                .field("outcome", outcome),
            Self::MobileContentAvailability { available } => debug
                .field("kind", &"mobile_content_availability")
                .field("available", available),
            Self::MobileSyncDocument(document) => debug
                .field("kind", &"mobile_sync_document")
                .field("has_document", &document.is_some()),
            Self::MobileSyncDocumentApplied(outcome) => debug
                .field("kind", &"mobile_sync_document_applied")
                .field("outcome", outcome),
            Self::MobileSyncFile(outcome) => debug
                .field("kind", &"mobile_sync_file")
                .field("outcome", outcome),
            Self::MobileFileUploadStarted(_) => debug.field("kind", &"mobile_file_upload_started"),
            Self::MobileFileUploadChunkAppended => {
                debug.field("kind", &"mobile_file_upload_chunk_appended")
            }
            Self::MobileFileUploadFinished(outcome) => debug
                .field("kind", &"mobile_file_upload_finished")
                .field("outcome", outcome),
            Self::MobileFileUploadAborted { existed } => debug
                .field("kind", &"mobile_file_upload_aborted")
                .field("existed", existed),
            Self::ReceiveReadiness(readiness) => debug
                .field("kind", &"receive_readiness")
                .field("readiness", readiness),
            Self::EncryptionState(state) => debug
                .field("kind", &"encryption_state")
                .field("state", state),
            Self::EncryptionLocked => debug.field("kind", &"encryption_locked"),
            Self::SecureStorageAccess { granted } => debug
                .field("kind", &"secure_storage_access")
                .field("granted", granted),
            Self::Devices(devices) => debug
                .field("kind", &"devices")
                .field("device_count", &devices.len()),
            Self::MemberSyncPreferences(preferences) => debug
                .field("kind", &"member_sync_preferences")
                .field("preferences", preferences),
            Self::WorkspaceMembership(summary) => debug
                .field("kind", &"workspace_convergence")
                .field("summary", summary),
            Self::DeviceTrust(summary) => debug
                .field("kind", &"device_trust")
                .field("revision", &summary.revision)
                .field("device_count", &summary.devices.len())
                .field("has_current_change", &summary.current_change.is_some()),
            Self::DeviceGroupChoices(summary) => debug
                .field("kind", &"device_group_choices")
                .field("issue_count", &summary.issues.len()),
            Self::DeviceGroupChosen(summary) => debug
                .field("kind", &"device_group_chosen")
                .field("outcome", &summary.outcome),
            #[cfg(feature = "dev-tools")]
            Self::MembershipDiagnostics(summary) => debug
                .field("kind", &"membership_diagnostics")
                .field("summary", summary),
            Self::SpaceProtection(summary) => debug
                .field("kind", &"space_protection")
                .field("summary", summary),
            Self::SearchPage(page) => debug.field("kind", &"search_page").field("page", page),
            Self::SearchTags(tags) => debug
                .field("kind", &"search_tags")
                .field("tag_count", &tags.len()),
            Self::SearchStatus(status) => debug
                .field("kind", &"search_status")
                .field("status", status),
            Self::SearchRebuildAccepted { accepted } => debug
                .field("kind", &"search_rebuild_accepted")
                .field("accepted", accepted),
            Self::EntrySent(report) => debug.field("kind", &"entry_sent").field("report", report),
            Self::HistoryPage {
                entries,
                next_cursor,
            } => debug
                .field("kind", &"history_page")
                .field("entry_count", &entries.len())
                .field("has_next_cursor", &next_cursor.is_some()),
            Self::HistoryEntries(entries) => debug
                .field("kind", &"history_entries")
                .field("entry_count", &entries.len()),
            Self::HistoryEntry(_) => debug.field("kind", &"history_entry"),
            Self::HistoryEntryDeleted => debug.field("kind", &"history_entry_deleted"),
            Self::HistoryEntryFavoriteSet => debug.field("kind", &"history_entry_favorite_set"),
            Self::HistoryStats(stats) => {
                debug.field("kind", &"history_stats").field("stats", stats)
            }
            Self::HistoryEntryResource(resource) => debug
                .field("kind", &"history_entry_resource")
                .field("has_blob", &resource.blob_id.is_some())
                .field("has_url", &resource.url.is_some())
                .field("has_inline_data", &resource.inline_data.is_some()),
            Self::BlobRead(resource) => debug
                .field("kind", &"blob_read")
                .field("byte_len", &resource.bytes.len()),
            Self::ThumbnailRead(resource) => debug
                .field("kind", &"thumbnail_read")
                .field("byte_len", &resource.bytes.len()),
            Self::EntryFileRead(resource) => debug
                .field("kind", &"entry_file_read")
                .field("byte_len", &resource.bytes.len())
                .field("has_file_name", &!resource.file_name.is_empty()),
            Self::EntryDelivery(view) => debug.field("kind", &"entry_delivery").field("view", view),
            Self::HistoryCleared(result) => debug
                .field("kind", &"history_cleared")
                .field("deleted_count", &result.deleted_count)
                .field("failed_count", &result.failed_entry_ids.len()),
            Self::EntryReceiveProgress(progress) => debug
                .field("kind", &"entry_receive_progress")
                .field("has_progress", &progress.is_some()),
            Self::EntryReceiveProgressList(progress) => debug
                .field("kind", &"entry_receive_progress_list")
                .field("progress_count", &progress.len()),
            Self::EntryReceiveCancellation(outcome) => debug
                .field("kind", &"entry_receive_cancellation")
                .field("outcome", outcome),
            Self::InboundTransferCancellation(outcome) => debug
                .field("kind", &"inbound_transfer_cancellation")
                .field("outcome", outcome),
            Self::ClipboardCaptured { entry_id } => debug
                .field("kind", &"clipboard_captured")
                .field("has_entry", &entry_id.is_some()),
            Self::ClipboardChangeObserved { report } => debug
                .field("kind", &"clipboard_change_observed")
                .field("has_report", &report.is_some()),
            Self::ActiveClipboard(active) => debug
                .field("kind", &"active_clipboard")
                .field("has_active", &active.is_some()),
            Self::ClipboardRestored(outcome) => debug
                .field("kind", &"clipboard_restored")
                .field("outcome", outcome),
            Self::EntryExported => debug.field("kind", &"entry_exported"),
            Self::EntryResent(outcome) => debug
                .field("kind", &"entry_resent")
                .field("outcome", outcome),
        };
        debug.finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SetupInvitationSummary {
    pub invitation_code: String,
    pub full_invitation: String,
    pub expires_at_ms: i64,
}

impl fmt::Debug for SetupInvitationSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SetupInvitationSummary")
            .field("invitation_code", &"[REDACTED]")
            .field("full_invitation", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SetupStateSummary {
    pub has_completed: bool,
    pub space_id: Option<String>,
    pub current_invitation: Option<SetupInvitationSummary>,
    pub device_name: Option<String>,
    pub re_pairing_required: bool,
}

impl fmt::Debug for SetupStateSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SetupStateSummary")
            .field("has_completed", &self.has_completed)
            .field("has_space_id", &self.space_id.is_some())
            .field("has_current_invitation", &self.current_invitation.is_some())
            .field("has_device_name", &self.device_name.is_some())
            .field("re_pairing_required", &self.re_pairing_required)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageStatsSummary {
    pub total_bytes: u64,
    pub database_bytes: u64,
    pub vault_bytes: u64,
    pub cache_bytes: u64,
    pub logs_bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDeviceSummary {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerConnectionChannelSummary {
    Direct,
    Relay,
    Offline,
    Unknown,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerConnectionSummary {
    pub peer_id: String,
    pub device_name: Option<String>,
    pub addresses: Vec<String>,
    pub is_paired: bool,
    pub connected: bool,
    pub pairing_state: String,
    pub channel: PeerConnectionChannelSummary,
    pub connection_address: Option<String>,
}

impl fmt::Debug for PeerConnectionSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerConnectionSummary")
            .field("has_device_name", &self.device_name.is_some())
            .field("address_count", &self.addresses.len())
            .field("is_paired", &self.is_paired)
            .field("connected", &self.connected)
            .field("has_pairing_state", &!self.pairing_state.is_empty())
            .field("channel", &self.channel)
            .field("has_connection_address", &self.connection_address.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerConnectionRefreshSummary {
    pub total: u32,
    pub online: u32,
    pub offline: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionStateSummary {
    pub initialized: bool,
    pub session_ready: bool,
}

impl fmt::Debug for LocalDeviceSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalDeviceSummary")
            .field("device_id", &self.device_id)
            .field("has_display_name", &!self.display_name.is_empty())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub device_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceMembershipSummary {
    Active,
    Removed,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceReachabilitySummary {
    Online,
    Offline,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceGroupRelationshipSummary {
    Consistent,
    PendingLocalDecision,
    Diverged,
    Unverifiable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCompatibilitySummary {
    Compatible,
    UpgradeRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSyncRelationshipSummary {
    Usable,
    WaitingForLocalDecision,
    PausedGroupDiverged,
    PausedUpgradeRequired,
    PausedUnverifiable,
    RemovedLocalDevice,
    RemovedPeerDevice,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTrustActionSummary {
    ApplyCurrentChange,
    KeepCurrentDeviceGroup,
    ConfirmApplyRemovesLocalDevice,
    RejoinDeviceGroup,
    UpdateThisDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTrustUnavailableReasonSummary {
    NoCurrentChange,
    ChangeNoLongerCurrent,
    LocalDeviceConfirmationRequired,
    LocalDeviceRemoved,
    RecoveryNotAvailableInThisVersion,
    PeerUpgradeRequired,
    DeviceFactsUnverifiable,
    EngineUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTrustRecoverySummary {
    NotAvailableInThisVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTrustImpactSummary {
    pub usable_device_ids: Vec<String>,
    pub paused_device_ids: Vec<String>,
    pub local_device_outcome: DeviceMembershipSummary,
    pub requires_rejoin_device_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTrustChangeSummary {
    pub change_id: String,
    pub proposed_by_device_id: String,
    pub target_device_ids: Vec<String>,
    pub includes_local_device: bool,
    pub apply_impact: DeviceTrustImpactSummary,
    pub keep_current_impact: DeviceTrustImpactSummary,
    pub allowed_choices: Vec<crate::DeviceTrustChoiceSummary>,
    pub blocked_reason: Option<DeviceTrustUnavailableReasonSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTrustRelationshipSummary {
    pub device_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub reachability: DeviceReachabilitySummary,
    pub membership: DeviceMembershipSummary,
    pub group_relationship: DeviceGroupRelationshipSummary,
    pub compatibility: DeviceCompatibilitySummary,
    pub sync_relationship: DeviceSyncRelationshipSummary,
    pub available_actions: Vec<DeviceTrustActionSummary>,
    pub blocked_reason: Option<DeviceTrustUnavailableReasonSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInboundMemberSummary {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTrustSnapshotSummary {
    pub revision: u64,
    pub local_device_id: String,
    pub local_membership: DeviceMembershipSummary,
    pub current_change: Option<DeviceTrustChangeSummary>,
    pub current_join: Option<JoinSpaceStatusSummary>,
    pub pending_inbound_member: Option<PendingInboundMemberSummary>,
    pub devices: Vec<DeviceTrustRelationshipSummary>,
    pub recovery: DeviceTrustRecoverySummary,
    pub allowed_actions: Vec<DeviceTrustActionSummary>,
    pub blocked_reason: Option<DeviceTrustUnavailableReasonSummary>,
    pub updated_at_ms: i64,
}

impl DeviceTrustSnapshotSummary {
    pub fn empty_unavailable(local_device_id: String) -> Self {
        Self {
            revision: 0,
            local_device_id,
            local_membership: DeviceMembershipSummary::Unavailable,
            current_change: None,
            current_join: None,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: DeviceTrustRecoverySummary::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: Some(DeviceTrustUnavailableReasonSummary::EngineUnavailable),
            updated_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConvergencePhaseSummary {
    LocallyApplied,
    Converging,
    Complete,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConvergenceFailureCategorySummary {
    SpaceMismatch,
    ContinuityGap,
    IdentityMismatch,
    DigestConflict,
    Unauthorized,
    VersionIncompatible,
    NoEffectiveMembers,
    Storage,
}

/// 工作空间收敛的统一完整快照(ADR-016)。
///
/// 仅供诊断构建观察内部进度；不含设备名称、成员实例、地址、签名、密钥、
/// 安全变化或内容。正式产品使用完整设备信任结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConvergenceSummary {
    pub phase: WorkspaceConvergencePhaseSummary,
    /// 只随成功持久化状态变化递增的不透明版本。
    pub revision: u64,
    /// 已保存的签名成员历史条目数量。
    pub history_event_count: u64,
    /// 当前有效成员实例数量。
    pub effective_member_count: u64,
    /// 已收到成员历史但仍等待本机决定移除的设备稳定标识，按字典序排列。
    pub pending_removal_decision_device_ids: Vec<String>,
    /// 提交接受或拒绝时使用的稳定待决定标识；没有待决定项时为空。
    pub pending_removal_decision_event_id: Option<String>,
    /// 已确认成员历史分叉、因此不再进行普通交换的设备稳定标识，按字典序排列。
    pub diverged_peer_device_ids: Vec<String>,
    /// 已确认仍低于 1.1、需要升级后才能继续同步的设备稳定标识，按字典序排列。
    pub upgrade_required_peer_device_ids: Vec<String>,
    /// 当前工作空间摘要;尚未形成时为空。
    pub convergence_digest: Option<String>,
    /// 本机当前成员实例是否已经观察到自己被移出。
    pub removed: bool,
    /// 最近一次成功保存状态的时间。
    pub updated_at_ms: i64,
    /// 可选的稳定失败类别,不含底层错误原文。
    pub failure_category: Option<WorkspaceConvergenceFailureCategorySummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceProtectionModeSummary {
    Legacy,
    Migrating,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberProtectionStatusSummary {
    LegacyUnprotected,
    Protected,
    AwaitingReadmission,
    RequiresReadmission,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProtectionSummary {
    pub device_id: String,
    pub status: MemberProtectionStatusSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceProtectionSummary {
    pub mode: SpaceProtectionModeSummary,
    pub members: Vec<MemberProtectionSummary>,
}

impl fmt::Debug for DeviceSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceSummary")
            .field("device_id", &self.device_id)
            .field("has_display_name", &!self.display_name.is_empty())
            .field("is_local", &self.is_local)
            .field("online", &self.online)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSyncPreferencesSummary {
    pub send_enabled: bool,
    pub receive_enabled: bool,
    pub send_content_types: ContentTypesSummary,
    pub receive_content_types: ContentTypesSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentTypesSummary {
    pub text: bool,
    pub image: bool,
    pub link: bool,
    pub file: bool,
    pub code_snippet: bool,
    pub rich_text: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPageSummary {
    pub total: u32,
    pub has_more: bool,
    pub items: Vec<SearchResultSummary>,
    pub state: String,
}

impl fmt::Debug for SearchPageSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchPageSummary")
            .field("total", &self.total)
            .field("has_more", &self.has_more)
            .field("item_count", &self.items.len())
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResultSummary {
    pub entry_id: String,
    pub content_type: String,
    pub active_time_ms: i64,
    pub tags: Vec<String>,
    pub text_preview: Option<String>,
    pub char_count: Option<i64>,
    pub mime_type: String,
    pub file_extensions: Vec<String>,
    pub file_names: Vec<String>,
    pub file_paths: Vec<String>,
    pub link_urls: Vec<String>,
    pub source_device: Option<String>,
    pub payload_state: Option<String>,
}

impl fmt::Debug for SearchResultSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchResultSummary")
            .field("entry_id", &self.entry_id)
            .field("content_type", &self.content_type)
            .field("active_time_ms", &self.active_time_ms)
            .field("tag_count", &self.tags.len())
            .field("has_text_preview", &self.text_preview.is_some())
            .field("char_count", &self.char_count)
            .field("file_count", &self.file_names.len())
            .field("link_count", &self.link_urls.len())
            .field("has_source_device", &self.source_device.is_some())
            .field("has_payload_state", &self.payload_state.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchTagSummary {
    pub tag_id: String,
    pub count: u32,
    pub is_builtin: bool,
}

impl fmt::Debug for SearchTagSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchTagSummary")
            .field("count", &self.count)
            .field("is_builtin", &self.is_builtin)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchStatusSummary {
    pub state: String,
    pub reason: Option<String>,
    pub last_rebuild_started_at_ms: Option<i64>,
    pub last_rebuild_completed_at_ms: Option<i64>,
}

impl fmt::Debug for SearchStatusSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchStatusSummary")
            .field("state", &self.state)
            .field("has_reason", &self.reason.is_some())
            .field(
                "last_rebuild_started_at_ms",
                &self.last_rebuild_started_at_ms,
            )
            .field(
                "last_rebuild_completed_at_ms",
                &self.last_rebuild_completed_at_ms,
            )
            .finish()
    }
}
