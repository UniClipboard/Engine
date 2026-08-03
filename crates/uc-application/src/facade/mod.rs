//! Slice 1 application facade tree.
//!
//! Per `uc-application/AGENTS.md` §11.4 external consumers only see the
//! top-level `AppFacade` and the per-domain sub-facades it aggregates.
//! Use cases live under `crate::usecases::<domain>` and stay `pub(crate)`;
//! sub-facades expose them through domain-scoped methods.

pub mod active_clipboard;
pub mod app_facade;
pub mod app_paths;
pub mod blob_transfer;
pub mod clipboard;
pub mod clipboard_capture;
pub mod clipboard_history;
pub mod clipboard_inbound;
pub mod clipboard_live_index;
pub mod clipboard_outbound;
pub mod clipboard_restore;
pub mod config_migration;
pub mod device;
pub mod diagnostics;
pub mod encryption;
pub mod file_transfer;
pub mod host_event;
pub mod legacy_upgrade;
mod membership_connectivity;
pub mod membership_gossip;
#[cfg(feature = "lan-compat")]
pub mod mobile_sync;
pub mod resource;
pub mod roster;
pub mod search;
pub mod settings;
pub mod setup_status;
mod space_admission;
mod space_runtime;
mod space_session;
pub mod space_setup;
pub mod storage;
pub mod upgrade;

pub use active_clipboard::{
    build_active_clipboard_pull_serve_port, ActiveClipboardDeps, ActiveClipboardFacade,
    ActiveClipboardLifecycle, ActiveClipboardLifecycleError, ActiveClipboardPullServeFacadeDeps,
    ActiveClipboardReconcileDeps, ActiveClipboardReconcileFacade, ActiveClipboardReconcileOutcome,
    ClipboardSnapshotDeps,
};
pub use app_facade::{
    AppFacade, AppFacadeParts, AppPresenceEvent, AppPresenceSubscription,
    AppPresenceSubscriptionError, ClipboardRestoreMode,
};
pub use app_paths::AppPaths;
pub use blob_transfer::{
    BlobTransferDeps, BlobTransferError, BlobTransferFacade, FetchBlobCommand, FetchBlobResult,
    FetchBlobToPathCommand, FetchBlobToPathResult, FetchTransferContext, InboundCancelOutcome,
    PublishBlobCommand, PublishBlobPathCommand, PublishBlobResult,
};
pub use clipboard::{
    CancelEntryReceiveError, CancelEntryReceiveOutcome, ClipboardSyncDeps, ClipboardSyncError,
    ClipboardSyncFacade, DispatchEntryInput, DispatchEntryOutcome, DispatchEntryPerTarget,
    EntryDeliveryStatusView, EntryDeliveryTargetView, EntryDeliveryView, EntrySource,
    GetEntryDeliveryViewError,
};
// V3 envelope codec helpers — surfaced through the facade per §11.4.3 so
// external CLI / test consumers don't reach into `crate::usecases::*`
// directly. Implementations live in `usecases::clipboard_sync::payload_codec`.
pub use crate::usecases::clipboard_sync::{
    decode_v3_bytes_to_snapshot, decode_v3_bytes_to_snapshot_and_blob_refs, V3BlobRef,
};
pub use clipboard_capture::{
    CapturedClipboardEntryView, CapturedFileSetLineView, CapturedFileSetView,
    ClipboardCaptureFacade, ClipboardCaptureFacadeError, ClipboardCapturePort,
};
pub use clipboard_history::{
    CleanupResultView as ClipboardCleanupResultView,
    ClearHistoryResultView as ClipboardClearHistoryResultView, ClipboardHistoryError,
    ClipboardHistoryFacade, ClipboardHistoryFacadeDeps, ClipboardListInput, ClipboardStatsView,
    EntryDetailView, EntryProjectionView, EntryResourceView, HistoryMaintenanceRuntime,
    HistoryMaintenanceRuntimeError, ReconcileResultView as ClipboardReconcileResultView,
};
pub use clipboard_inbound::{
    ClipboardInboundEvent, ClipboardInboundEventAction, ClipboardInboundEventPort,
    ClipboardInboundRepresentationSummary, ClipboardInboundRuntime, ClipboardInboundRuntimeDeps,
    ClipboardInboundRuntimeError, InboundClipboardApplyError, InboundClipboardApplyInput,
    InboundClipboardApplyOutcome, InboundClipboardApplyPort,
};
pub use clipboard_live_index::{
    ClipboardLiveIndexDeps, ClipboardLiveIndexError, ClipboardLiveIndexFacade,
    ClipboardLiveIndexInput, ClipboardLiveIndexOutcome, ClipboardLiveIndexPort,
    ClipboardLiveIndexer,
};
pub use clipboard_outbound::{
    ClipboardOutboundDeps, ClipboardOutboundDispatcher, ClipboardOutboundError,
    ClipboardOutboundFacade, ClipboardOutboundInput, ClipboardOutboundOutcome,
    ClipboardOutboundPort, NotResendableReason, ResendEntryCommand, ResendEntryError, ResendReport,
    MAX_INLINE_OUTBOUND_REPRESENTATION_BYTES,
};
pub use clipboard_restore::{
    ClipboardRestoreError, ClipboardRestoreFacade, ClipboardRestoreFacadeDeps,
};
pub use config_migration::{ConfigMigrationDeps, ConfigMigrationFacade};
pub use device::{DeviceFacade, DeviceFacadeError, LocalDeviceInfoView};
pub use diagnostics::{
    DebugStatusView, DiagnosticsFacade, DiagnosticsFacadeDeps, DiagnosticsFacadeError,
    LogExportView, UpdateDebugModeView,
};
pub use encryption::{
    EncryptionFacade, EncryptionFacadeDeps, EncryptionFacadeError, EncryptionStateView,
};
pub use file_transfer::{
    BeginReceiverTransfer, FileTransferApplicationError, FileTransferFacade,
    FileTransferFacadeDeps, FileTransferSession, ReceiverTransferRegistration,
};
pub use host_event::{
    ClipboardHostEvent, ClipboardOriginKind, DeliveryHostEvent, EmitError,
    FileTransferHostEventPublisher, HostEvent, HostEventBus, HostEventEmitterPort,
    OutboundEntryIdCache, TransferHostEvent,
};
pub use legacy_upgrade::{
    AutomaticLegacyUpgrade, AutomaticLegacyUpgradeDeps, AutomaticLegacyUpgradeRuntime,
};
pub use membership_connectivity::{
    start_membership_connectivity, MembershipConnectivityDeps, MembershipConnectivityRuntime,
};
pub use membership_gossip::{
    build_space_membership_gossip, MembershipConvergenceState, MembershipConvergenceStatus,
    MembershipGossipPassOutcome, MembershipGossipRuntimeError, PairingMembershipGossipPort,
    SpaceMembershipGossip, SpaceMembershipGossipActivity, SpaceMembershipGossipDeps,
    SpaceMembershipGossipError, SpaceMembershipGossipRuntime, SponsorSeedBatchContext,
};
#[cfg(feature = "lan-compat")]
pub use mobile_sync::{
    ApplyIncomingMobileClipError, ApplyIncomingMobileClipInput, ApplyIncomingMobileClipOutcome,
    AuthenticateBasicAuthError, AuthenticateBasicAuthInput, AuthenticatedDevice,
    BeginMobileFileUpload, CheckContentAvailableError, GetLatestMobileSyncDocError,
    GetMobileSyncFileError, GetMobileSyncFileOutput, GetMobileSyncSettingsError,
    IncomingMobileBuffer, IncomingMobileClipEvent,
    LanInterfaceOption as MobileSyncLanInterfaceOption,
    ListLanInterfacesError as MobileSyncListLanInterfacesError, ListMobileDevicesError,
    MobileDeviceSummary, MobileFileUploadError, MobileFileUploadHandle, MobileSyncFacade,
    MobileSyncFacadeDeps, MobileSyncSettingsView, MobileSyncSnapshotPorts,
    RegisterMobileShortcutDeviceError, RegisterMobileShortcutDeviceInput,
    RegisterMobileShortcutDeviceOutput, RevokeMobileDeviceError, RevokeMobileDeviceInput,
    ShortcutInstallMethod, ShortcutInstallMethodOption, SyncClipboardItemType, SyncClipboardMeta,
    UpdateMobileSyncSettingsError, UpdateMobileSyncSettingsInput, UpdateMobileSyncSettingsOutput,
    SYNC_CLIPBOARD_EX_INSTALL_URL,
};
pub use resource::{
    BinaryResourceView, FileResourceView, ResourceFacade, ResourceFacadeDeps, ResourceFacadeError,
};
pub use roster::{
    connection_channel_to_wire, ConnectionChannel, ContentTypesPatch, ContentTypesView,
    LegacyBootstrapState, LegacyBootstrapView, MemberProtectionStatusView, MemberProtectionView,
    MemberRevocationState, MemberRevocationView, MemberRosterDeps, MemberRosterFacade,
    MemberSummary, MemberSyncPreferencesPatch, MemberSyncPreferencesView, PeerSnapshotView,
    PresenceEvent, RosterEntry, RosterError, SpaceProtectionModeView, SpaceProtectionView,
};
pub use search::{
    map_search_error, SearchFacade, SearchFacadeError, SearchPageView, SearchProjectionBuilder,
    SearchQueryInput, SearchRebuildAcceptedView, SearchRebuildProgressView, SearchResultView,
    SearchRuntime, SearchRuntimeDeps, SearchRuntimeError, SearchStatusSnapshot, SearchStatusView,
    SearchTagView,
};
// Note: `RelayDiagnosticPort` is intentionally NOT re-exported here. The port
// trait stays under `crate::facade::settings::relay_diagnostic` and is reached
// via `uc_application::facade::settings::RelayDiagnosticPort` by bootstrap,
// keeping the assembly seam scoped to the settings sub-facade (per §11.4).
pub use settings::{
    ContentTypesPatch as SettingsContentTypesPatch, ContentTypesView as SettingsContentTypesView,
    FileSyncSettingsPatch, FileSyncSettingsView, GeneralSettingsPatch, GeneralSettingsView,
    PairingSettingsPatch, PairingSettingsView, RelayProbeError, RelayProbeReport,
    RelayProbeReportView, RetentionPolicyPatch, RetentionPolicyView, RetentionRulePatchValue,
    RetentionRuleView, RuleEvaluationView, SecuritySettingsPatch, SecuritySettingsView,
    SettingsFacade, SettingsFacadeError, SettingsPatch, SettingsView, ShortcutKeyView,
    SyncFrequencyView, SyncSettingsPatch, SyncSettingsView, ThemeView, UpdateChannelView,
};
pub use setup_status::SetupStatusFacade;
pub use space_admission::{JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
pub use space_runtime::{
    MembershipConvergenceFacadeError, SpaceApplicationHandle, SpaceApplicationRuntime,
};
pub use space_session::{
    RecoverSpaceSessionResult, SpaceActivityError, SpaceSessionAccessDeps,
    SpaceSessionActivityDeps, SpaceSessionError,
};
pub use space_setup::{
    CancelInvitationError, CurrentInvitation, FactoryResetError, InitializeSpaceError,
    InitializeSpaceInput, InitializeSpaceResult, IssuePairingInvitationError,
    IssuePairingInvitationResult, PairingFailureReason, PairingInvitationAddressCandidate,
    PairingOutcome, QuerySetupStateError, RedeemPairingInvitationError,
    RedeemPairingInvitationInput, RedeemPairingInvitationResult, ResetSpaceError, SetupStateView,
    SpaceAdmissionDeps, SpaceFacade, SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps,
    UnlockSpaceError, UnlockSpaceInput, UnlockSpaceResult,
};
pub use storage::{
    ClearCacheResultView, StorageFacade, StorageFacadeDeps, StorageFacadeError, StorageStatsView,
};
pub use upgrade::{
    AcknowledgeUpgradeError, DetectUpgradeError, UpgradeFacade, UpgradeFacadeDeps, UpgradeStatus,
};
