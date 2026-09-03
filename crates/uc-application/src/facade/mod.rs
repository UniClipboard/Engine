//! Slice 1 application facade tree.
//!
//! Per `docs/design-docs/layers/application.md` external consumers only see the
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

pub use crate::application::{
    ApplicationAssembly, ApplicationRuntime, ApplicationRuntimeError, ApplicationShutdownReport,
    ApplicationStartError, ApplicationUpgradeError,
};

pub use crate::device::query_local_device::LocalDeviceInfo;
pub use crate::profile::factory_reset::{
    ProfileFactoryResetError, ProfileFactoryResetFacade, ProfileFactoryResetOutcome,
    ProfileFactoryResetRequest,
};
pub use crate::profile::probe_profile_key_access::ProbeProfileKeyAccessError;
pub use crate::space::{
    CancelSpaceJoinError, CurrentJoinStatus, DecideDeviceTrustChange, DecideDeviceTrustChangeError,
    DecideDeviceTrustChangeResult, DeviceTrustChangeChoice, DeviceTrustDevice,
    DeviceTrustMembership, DeviceTrustObservation, DeviceTrustRelationship, DeviceTrustStatus,
    DeviceTrustSyncState, JoinSpaceError, JoinSpaceInput, JoinSpaceResult, JoinedSpace,
    LockSpaceSessionError, MembershipCommitReceipt, MembershipConflictStatus,
    MembershipDiagnosticsView, NetworkRecoveryEvent, NetworkRecoveryFacade, NetworkRecoveryPhase,
    NetworkRecoveryRequestError, NetworkRecoveryStatus, PendingDeviceTrustChange,
    PendingInboundMember, QueryDeviceTrustError, QueryMembershipDiagnosticsError,
    QuerySpaceAccessStateError, RebuildNetworkSessionError, RebuildNetworkSessionPort,
    RecoverSpaceSessionError, RecoverSpaceSessionResult, RemoveSpaceMemberError,
    RemoveSpaceMemberResult, SpaceAccessState,
};

pub use crate::clipboard::active::{ActiveClipboardFacade, ActiveClipboardReconcileOutcome};
pub use app_facade::{
    AppFacade, AppPresenceEvent, AppPresenceSubscription, AppPresenceSubscriptionError,
    ChooseDeviceGroup, ChooseDeviceGroupError, ChooseDeviceGroupResult, ClipboardRestoreMode,
    DeviceGroupChoice, DeviceGroupChoicesView, DeviceGroupIssue, QueryDeviceGroupChoicesError,
};
pub use app_paths::AppPaths;
pub use blob_transfer::{
    BlobTransferError, BlobTransferFacade, FetchBlobCommand, FetchBlobResult,
    FetchBlobToPathCommand, FetchBlobToPathResult, FetchTransferContext, InboundCancelOutcome,
    PublishBlobCommand, PublishBlobPathCommand, PublishBlobResult,
};
pub use clipboard::{
    CancelEntryReceiveError, CancelEntryReceiveOutcome, ClipboardSyncError, ClipboardSyncFacade,
    DispatchEntryInput, DispatchEntryOutcome, DispatchEntryPerTarget, EntryDeliveryStatusView,
    EntryDeliveryTargetView, EntryDeliveryView, EntrySource, GetEntryDeliveryViewError,
};
// V3 envelope codec helpers — surfaced through the facade per §11.4.3 so
// external CLI / test consumers don't reach into `crate::usecases::*`
// directly. Implementations live in `usecases::clipboard_sync::payload_codec`.
pub use crate::clipboard::inbound::{
    ClipboardInboundEvent, ClipboardInboundEventAction, ClipboardInboundEventPort,
    ClipboardInboundRepresentationSummary, ClipboardInboundRuntimeError,
    InboundClipboardApplyError, InboundClipboardApplyInput, InboundClipboardApplyOutcome,
    InboundClipboardApplyPort, InboundProvisionalReceive,
};
pub use crate::clipboard::outbound::{
    ClipboardOutboundError, ClipboardOutboundFacade, ClipboardOutboundInput,
    ClipboardOutboundOutcome, ClipboardOutboundPort, NotResendableReason, ResendEntryCommand,
    ResendEntryError, ResendReport, MAX_INLINE_OUTBOUND_REPRESENTATION_BYTES,
};
pub use crate::clipboard::sync::apply_inbound::{
    ApplyInboundError, ApplyInboundInput, ApplyOutcome, FileCacheBlobMaterializer,
    InboundBlobFetcher, InboundCapture, InboundSnapshotRebuild, InboundWrite,
};
pub use crate::clipboard::sync::payload_codec::{self, encode_snapshot_to_v3_bytes};
pub use crate::clipboard::sync::{
    decode_v3_bytes_to_snapshot, decode_v3_bytes_to_snapshot_and_blob_refs, V3BlobRef,
};
pub use crate::search::live_index::{
    ClipboardLiveIndexError, ClipboardLiveIndexFacade, ClipboardLiveIndexInput,
    ClipboardLiveIndexOutcome,
};
pub use crate::transfer::receive::reconciliation::{
    EnsureReceiveReadyPort, ReceiveReadinessError, ReceiveReadinessStatus,
};
pub use clipboard_capture::{
    CapturedClipboardEntryView, CapturedFileSetLineView, CapturedFileSetView,
    ClipboardCaptureFacade, ClipboardCaptureFacadeError, ClipboardCapturePort,
};
pub use clipboard_history::{
    CleanupResultView as ClipboardCleanupResultView,
    ClearHistoryResultView as ClipboardClearHistoryResultView, ClipboardHistoryError,
    ClipboardHistoryFacade, ClipboardListInput, ClipboardStatsView, EntryDetailView,
    EntryProjectionView, EntryResourceView, ReconcileResultView as ClipboardReconcileResultView,
};
pub use clipboard_restore::{ClipboardRestoreError, ClipboardRestoreFacade};
pub use config_migration::ConfigMigrationFacade;
pub use diagnostics::{
    DebugStatusView, DiagnosticsFacade, DiagnosticsFacadeError, LogExportView, UpdateDebugModeView,
};
pub use file_transfer::{
    BeginReceiverTransfer, FileTransferApplicationError, FileTransferFacade,
    ReceiverTransferHandle, ReceiverTransferRegistration,
};
pub use host_event::{
    ClipboardHostEvent, ClipboardOriginKind, DeliveryHostEvent, EmitError,
    FileTransferHostEventPublisher, HostEvent, HostEventBus, HostEventEmitterPort,
    OutboundEntryIdCache, TransferHostEvent,
};

pub use crate::clipboard::resource::{
    BinaryResourceView, FileResourceView, ResourceFacade, ResourceFacadeError,
};
pub use roster::{
    connection_channel_to_wire, ConnectionChannel, ContentTypesPatch, ContentTypesView,
    MemberProtectionStatusView, MemberProtectionView, MemberSummary, MemberSyncPreferencesPatch,
    MemberSyncPreferencesView, PeerReachabilityChanged, PeerSnapshotView, PresenceRefreshReport,
    RosterEntry, RosterError, SpaceProtectionModeView, SpaceProtectionView,
};
pub use search::{
    map_search_error, SearchFacade, SearchFacadeError, SearchPageView, SearchProjectionBuilder,
    SearchQueryInput, SearchRebuildAcceptedView, SearchRebuildProgressView, SearchResultView,
    SearchShutdownError, SearchStatusSnapshot, SearchStatusView, SearchTagView,
};
// Note: `RelayDiagnosticPort` is intentionally NOT re-exported here. The port
// trait stays under `crate::facade::settings::relay_diagnostic` and is reached
// via `uc_application::facade::settings::RelayDiagnosticPort` by bootstrap,
// keeping the assembly seam scoped to the settings sub-facade (per §11.4).
pub use settings::{
    ContentTypesPatch as SettingsContentTypesPatch, ContentTypesView as SettingsContentTypesView,
    FileSyncSettingsPatch, FileSyncSettingsView, GeneralSettingsPatch, GeneralSettingsView,
    PairingSettingsPatch, PairingSettingsView, PreparedNetworkSettings, RelayProbeError,
    RelayProbeReport, RelayProbeReportView, RetentionPolicyPatch, RetentionPolicyView,
    RetentionRulePatchValue, RetentionRuleView, RuleEvaluationView, SecuritySettingsPatch,
    SecuritySettingsView, SettingsFacade, SettingsFacadeError, SettingsPatch, SettingsView,
    ShortcutKeyView, SyncFrequencyView, SyncSettingsPatch, SyncSettingsView, ThemeView,
    UpdateChannelView,
};

pub use space_setup::{
    CancelInvitationError, CompletePendingSpaceTransitionError, CurrentInvitation,
    InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult, IssuePairingInvitationError,
    IssuePairingInvitationResult, MembershipConflictBranchView, MembershipConflictView,
    MembershipConflictsView, PairingInvitationAddressCandidate, QueryMembershipConflictsError,
    QueryPairingInvitationAddressesError, QueryPendingSpaceTransitionError, QuerySetupStateError,
    RedeemPairingInvitationError, ResetSpaceError, ResolveMembershipConflictError,
    ResolveMembershipConflictInput, ResolveMembershipConflictResult, SetupStateView,
    SpaceActivityError, SpaceFacade, UnlockSpaceError, UnlockSpaceInput, UnlockSpaceResult,
};
pub use storage::{ClearCacheResultView, StorageFacade, StorageFacadeError, StorageStatsView};
pub use upgrade::{AcknowledgeUpgradeError, DetectUpgradeError, UpgradeFacade, UpgradeStatus};
