//! Slice 1 application facade tree.
//!
//! Per `uc-application/AGENTS.md` §11.4 external consumers only see the
//! top-level `AppFacade` and the per-domain sub-facades it aggregates.
//! Use cases live under `crate::usecases::<domain>` and stay `pub(crate)`;
//! sub-facades expose them through domain-scoped methods.

pub mod app_facade;
pub mod app_paths;
pub mod blob_transfer;
pub mod clipboard;
pub mod clipboard_capture;
pub mod clipboard_history;
pub mod clipboard_restore;
pub mod clipboard_write;
pub mod config_migration;
pub mod diagnostics;
pub mod file_transfer;
pub mod host_event;
pub mod roster;
pub mod search;
pub mod settings;
pub mod space_setup;
pub mod storage;
pub mod upgrade;

pub use crate::device::query_local_device::{LocalDeviceInfo, QueryLocalDeviceUseCase};
pub use crate::profile::factory_reset::{
    ProfileFactoryResetError, ProfileFactoryResetOutcome, ProfileFactoryResetRequest,
    ProfileFactoryResetUseCase,
};
pub use crate::profile::probe_profile_key_access::{
    ProbeProfileKeyAccessError, ProbeProfileKeyAccessUseCase,
};
pub use crate::space::{
    CancelSpaceJoinError, CurrentJoinStatus, DecideDeviceTrustChange, DecideDeviceTrustChangeError,
    DecideDeviceTrustChangeResult, DeviceTrustChangeChoice, DeviceTrustDevice,
    DeviceTrustMembership, DeviceTrustObservation, DeviceTrustRelationship, DeviceTrustStatus,
    DeviceTrustSyncState, JoinSpaceError, JoinSpaceInput, JoinSpaceResult, JoinedSpace,
    LockSpaceSessionError, MembershipCommitReceipt, NetworkRecoveryEvent, NetworkRecoveryFacade,
    NetworkRecoveryPhase, NetworkRecoveryRequestError, NetworkRecoveryStatus,
    PendingDeviceTrustChange, PendingInboundMember, QueryDeviceTrustError,
    QuerySpaceAccessStateError, RebuildNetworkSessionError, RebuildNetworkSessionPort,
    RecoverSpaceSessionError, RecoverSpaceSessionResult, RemoveSpaceMemberError,
    RemoveSpaceMemberResult, SpaceAccessState,
};

pub use crate::clipboard::active::{
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
pub use crate::clipboard::inbound::{
    ClipboardInboundEvent, ClipboardInboundEventAction, ClipboardInboundEventPort,
    ClipboardInboundRepresentationSummary, ClipboardInboundRuntime, ClipboardInboundRuntimeDeps,
    ClipboardInboundRuntimeError, InboundClipboardApplyError, InboundClipboardApplyInput,
    InboundClipboardApplyOutcome, InboundClipboardApplyPort,
};
pub use crate::clipboard::outbound::{
    ClipboardOutboundDeps, ClipboardOutboundDispatcher, ClipboardOutboundError,
    ClipboardOutboundFacade, ClipboardOutboundInput, ClipboardOutboundOutcome,
    ClipboardOutboundPort, NotResendableReason, ResendEntryCommand, ResendEntryError, ResendReport,
    MAX_INLINE_OUTBOUND_REPRESENTATION_BYTES,
};
pub use crate::clipboard::sync::apply_inbound::{
    ApplyInboundClipboardUseCase, ApplyInboundError, ApplyInboundInput, ApplyOutcome,
    FileCacheBlobMaterializer, InboundApplyCommonDeps, InboundBlobFetcher, InboundCapture,
    InboundReceiveAttemptDeps, InboundSnapshotRebuild, InboundWrite, InteractiveReceiveDeps,
    StoreOnlyPullDeps,
};
pub use crate::clipboard::sync::payload_codec::{self, encode_snapshot_to_v3_bytes};
pub use crate::clipboard::sync::sync_runtime::{ClipboardSyncRuntime, ClipboardSyncRuntimeDeps};
pub use crate::clipboard::sync::{
    decode_v3_bytes_to_snapshot, decode_v3_bytes_to_snapshot_and_blob_refs, V3BlobRef,
};
pub use crate::search::live_index::{
    ClipboardLiveIndexDeps, ClipboardLiveIndexError, ClipboardLiveIndexFacade,
    ClipboardLiveIndexInput, ClipboardLiveIndexOutcome, ClipboardLiveIndexPort,
    ClipboardLiveIndexer,
};
pub use crate::transfer::receive::reconciliation::{
    EnsureReceiveReadyPort, ReceiveReadinessCoordinator, ReceiveReadinessError,
    ReceiveReadinessStatus,
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
pub use clipboard_restore::{
    ClipboardRestoreError, ClipboardRestoreFacade, ClipboardRestoreFacadeDeps,
};
pub use config_migration::{ConfigMigrationDeps, ConfigMigrationFacade};
pub use diagnostics::{
    DebugStatusView, DiagnosticsFacade, DiagnosticsFacadeDeps, DiagnosticsFacadeError,
    LogExportView, UpdateDebugModeView,
};
pub use file_transfer::{
    BeginReceiverTransfer, FileTransferApplicationError, FileTransferFacade,
    FileTransferFacadeDeps, FileTransferLifecycleDeps, FileTransferSession,
    ReceiverTransferRegistration,
};
pub use host_event::{
    ClipboardHostEvent, ClipboardOriginKind, DeliveryHostEvent, EmitError,
    FileTransferHostEventPublisher, HostEvent, HostEventBus, HostEventEmitterPort,
    OutboundEntryIdCache, TransferHostEvent,
};

pub use crate::clipboard::resource::{
    BinaryResourceView, FileResourceView, ResourceFacade, ResourceFacadeDeps, ResourceFacadeError,
};
pub use roster::{
    connection_channel_to_wire, ConnectionChannel, ContentTypesPatch, ContentTypesView,
    MemberProtectionStatusView, MemberProtectionView, MemberSummary, MemberSyncPreferencesPatch,
    MemberSyncPreferencesView, PeerSnapshotView, PresenceEvent, RosterEntry, RosterError,
    SpaceProtectionModeView, SpaceProtectionView,
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

pub use space_setup::{
    CancelInvitationError, CompletePendingSpaceTransitionError, CurrentInvitation,
    InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult, IssuePairingInvitationError,
    IssuePairingInvitationResult, PairingInvitationAddressCandidate,
    QueryPairingInvitationAddressesError, QueryPendingSpaceTransitionError, QuerySetupStateError,
    RedeemPairingInvitationError, ResetSpaceError, SetupStateView, SpaceAdmissionDeps, SpaceFacade,
    SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps, UnlockSpaceError, UnlockSpaceInput,
    UnlockSpaceResult,
};
pub use storage::{
    ClearCacheResultView, StorageFacade, StorageFacadeDeps, StorageFacadeError, StorageStatsView,
};
pub use upgrade::{
    AcknowledgeUpgradeError, DetectUpgradeError, UpgradeFacade, UpgradeFacadeDeps, UpgradeStatus,
};
