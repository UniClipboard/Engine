//! `AppFacade` — Slice 1 cross-domain aggregator.
//!
//! Per `uc-application/AGENTS.md` §11.4 external consumers reach the
//! application layer exclusively through a facade. `AppFacade` is the
//! single outward-facing type; internally it just groups sub-facades,
//! each constructed from its own `*Deps` bundle, so adding a new
//! domain does not cascade into a constructor explosion.
//!
//! # Current scope (Slice 1 · P4)
//!
//! * [`SpaceFacade`] — A1 `initialize_space`, A2 `unlock_space`
//!
//! # Deferred
//!
//! * `PairingFacade` (B1 / B2) → P7+
//! * `SyncFacade` (C1 / C2 / C3) → Slice 2
//! * F1 `on_startup` / F2 `on_shutdown` → P6 (lives inside the
//!   sub-facades once `StartNetwork` plumbing exists)
//! * Daemon / tauri / CLI switching from the legacy sub-facades
//!   (`SetupFacade`, `PairingFacade`) to `AppFacade` → Slice 1.5 or
//!   later. Those sub-facades remain `pub` this slice to keep existing
//!   entry points working. D18 retired the legacy access facade because
//!   its state machine had no dispatcher, while the real admit path runs
//!   through `PairingInboundOrchestrator`.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::broadcast;

use crate::facade::config_migration::ConfigMigrationFacade;
use crate::facade::network_recovery::{
    NetworkRecoveryFacade, NetworkRecoveryRequestError, NetworkRecoveryStatus,
};
use crate::facade::roster::{MemberSummary, PeerSnapshotView, RosterError};
use crate::facade::settings::{GeneralSettingsPatch, SettingsPatch};
use crate::facade::space_admission::SpaceAdmissionCoordinator;
use crate::facade::space_session::{
    build_space_session_coordinator, RecoverSpaceSessionResult, SpaceSessionAccessDeps,
    SpaceSessionActivityDeps, SpaceSessionCoordinator, SpaceSessionError,
};
use crate::facade::space_setup::{EnsureReachableAllError, EnsureReachableAllReport};
use crate::facade::space_setup::{
    InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult, IssuePairingInvitationError,
    IssuePairingInvitationResult, MigrationProgress, PairingInvitationAddressCandidate,
    PairingOutcome, QueryMigrationProgressError, QuerySetupStateError,
    RedeemPairingInvitationError, RedeemPairingInvitationInput, RedeemPairingInvitationResult,
    SetupStateView, SwitchSpaceError, SwitchSpaceInput, SwitchSpaceResult, TryResumeSessionError,
    UnlockSpaceError, UnlockSpaceInput, UnlockSpaceResult,
};
use crate::facade::upgrade::UpgradeFacade;
use crate::facade::SpaceApplicationHandle;
use crate::facade::{
    BlobTransferError, BlobTransferFacade, ClipboardCaptureFacade, ClipboardHistoryFacade,
    ClipboardOutboundFacade, ClipboardRestoreError, ClipboardRestoreFacade, ClipboardSyncError,
    ClipboardSyncFacade, DeviceFacade, DiagnosticsFacade, DispatchEntryOutcome, EncryptionFacade,
    EncryptionFacadeError, EncryptionStateView, FetchBlobCommand, FetchBlobResult,
    FetchBlobToPathCommand, FetchBlobToPathResult, HistoryMaintenanceRuntime, MemberRosterFacade,
    PublishBlobCommand, PublishBlobPathCommand, PublishBlobResult, ResendEntryCommand,
    ResendEntryError, ResendReport, ResourceFacade, SearchFacade, SearchFacadeError,
    SearchPageView, SearchQueryInput, SearchRebuildAcceptedView, SearchStatusView, SettingsFacade,
    SettingsFacadeError, SpaceFacade, StorageFacade,
};
use crate::usecases::clipboard_sync::V3BlobRef;
use uc_core::ids::DeviceId;
use uc_core::ports::{PresenceError, PresenceEvent, ReachabilityState};
use uc_core::ClipboardChangeOrigin;
use uc_core::SystemClipboardSnapshot;

/// 应用层统一入口。
///
/// 外部业务调用只能通过本文件中的顶层方法进入。生产装配一次提供全部能力，
/// 因此运行期拿到的对象始终可以立即处理所有稳定 Engine 动作。
pub struct AppFacade {
    space: Arc<SpaceFacade>,
    space_session: Arc<SpaceSessionCoordinator>,
    space_application: SpaceApplicationHandle,
    space_admission: Arc<SpaceAdmissionCoordinator>,
    member_roster: Arc<MemberRosterFacade>,
    encryption: Arc<EncryptionFacade>,
    resource: Arc<ResourceFacade>,
    clipboard_history: Arc<ClipboardHistoryFacade>,
    clipboard_capture: Arc<ClipboardCaptureFacade>,
    clipboard_sync: Arc<ClipboardSyncFacade>,
    blob_transfer: Arc<BlobTransferFacade>,
    clipboard_outbound: Arc<ClipboardOutboundFacade>,
    clipboard_restore: Arc<ClipboardRestoreFacade>,
    search: Arc<SearchFacade>,
    settings: Arc<SettingsFacade>,
    diagnostics: Arc<DiagnosticsFacade>,
    device: Arc<DeviceFacade>,
    storage: Arc<StorageFacade>,
    config_migration: Arc<ConfigMigrationFacade>,
    upgrade: Arc<UpgradeFacade>,
    network_recovery: Arc<NetworkRecoveryFacade>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRestoreMode {
    Standard,
    PlainText,
    FilePaths,
}

impl AppFacade {
    /// Compose from already-constructed sub-facades.
    ///
    /// Bootstrap builds each sub-facade from its own `*Deps` bundle and
    /// hands them here — the aggregator never sees raw ports.
    pub fn new(parts: AppFacadeParts) -> Self {
        let space_session = build_space_session_coordinator(
            Arc::clone(&parts.space),
            Arc::clone(&parts.search),
            parts.space_session_activity,
            parts.space_session_access,
        );
        let space_admission = Arc::new(SpaceAdmissionCoordinator::new(
            Arc::clone(&parts.space),
            Arc::clone(&parts.settings),
        ));
        Self {
            space: parts.space,
            space_session,
            space_application: parts.space_application,
            space_admission,
            member_roster: parts.member_roster,
            encryption: parts.encryption,
            resource: parts.resource,
            clipboard_history: parts.clipboard_history,
            clipboard_capture: parts.clipboard_capture,
            clipboard_sync: parts.clipboard_sync,
            blob_transfer: parts.blob_transfer,
            clipboard_outbound: parts.clipboard_outbound,
            clipboard_restore: parts.clipboard_restore,
            search: parts.search,
            settings: parts.settings,
            diagnostics: parts.diagnostics,
            device: parts.device,
            storage: parts.storage,
            config_migration: parts.config_migration,
            upgrade: parts.upgrade,
            network_recovery: parts.network_recovery,
        }
    }

    pub async fn start_history_maintenance(&self) -> HistoryMaintenanceRuntime {
        HistoryMaintenanceRuntime::start(Arc::clone(&self.clipboard_history)).await
    }

    pub async fn capture_current_clipboard(
        &self,
    ) -> Result<
        Option<crate::facade::CapturedClipboardEntryView>,
        crate::facade::ClipboardCaptureFacadeError,
    > {
        self.clipboard_capture.capture_current().await
    }

    pub async fn capture_file_paths_for_diagnostics(
        &self,
        paths: Vec<PathBuf>,
    ) -> Result<crate::facade::CapturedFileSetView, crate::facade::ClipboardCaptureFacadeError>
    {
        self.clipboard_capture
            .capture_file_paths_for_diagnostics(paths)
            .await
    }

    pub async fn restore_clipboard(
        &self,
        entry_id: &str,
        mode: ClipboardRestoreMode,
    ) -> Result<(), ClipboardRestoreError> {
        match mode {
            ClipboardRestoreMode::Standard => self.clipboard_restore.restore_entry(entry_id).await,
            ClipboardRestoreMode::PlainText => {
                self.clipboard_restore
                    .restore_entry_as_plain_text(entry_id)
                    .await
            }
            ClipboardRestoreMode::FilePaths => {
                self.clipboard_restore
                    .restore_entry_as_file_paths(entry_id)
                    .await
            }
        }
    }

    pub async fn list_history_entries(
        &self,
        input: crate::facade::ClipboardListInput,
    ) -> Result<Vec<crate::facade::EntryProjectionView>, crate::facade::ClipboardHistoryError> {
        self.clipboard_history.list_entries(input).await
    }

    pub async fn seed_history_text(
        &self,
        text: &str,
    ) -> Result<String, crate::facade::ClipboardHistoryError> {
        self.clipboard_history.seed_text_entry(text).await
    }

    pub async fn get_history_entry(
        &self,
        entry_id: &str,
    ) -> Result<crate::facade::EntryDetailView, crate::facade::ClipboardHistoryError> {
        self.clipboard_history.get_entry(entry_id).await
    }

    pub async fn delete_history_entry(
        &self,
        entry_id: &str,
    ) -> Result<(), crate::facade::ClipboardHistoryError> {
        self.clipboard_history.delete_entry(entry_id).await
    }

    pub async fn set_history_entry_favorite(
        &self,
        entry_id: &str,
        is_favorited: bool,
    ) -> Result<bool, crate::facade::ClipboardHistoryError> {
        self.clipboard_history
            .toggle_favorite(entry_id, is_favorited)
            .await
    }

    pub async fn history_stats(
        &self,
    ) -> Result<crate::facade::ClipboardStatsView, crate::facade::ClipboardHistoryError> {
        self.clipboard_history.stats().await
    }

    pub async fn get_history_entry_resource(
        &self,
        entry_id: &str,
    ) -> Result<crate::facade::EntryResourceView, crate::facade::ClipboardHistoryError> {
        self.clipboard_history.get_entry_resource(entry_id).await
    }

    pub async fn clear_history(
        &self,
    ) -> Result<crate::facade::ClipboardClearHistoryResultView, crate::facade::ClipboardHistoryError>
    {
        self.clipboard_history.clear_history().await
    }

    pub async fn read_blob_resource(
        &self,
        blob_id: &str,
    ) -> Result<crate::facade::BinaryResourceView, crate::facade::ResourceFacadeError> {
        self.resource.blob(blob_id).await
    }

    pub async fn read_thumbnail_resource(
        &self,
        representation_id: &str,
    ) -> Result<crate::facade::BinaryResourceView, crate::facade::ResourceFacadeError> {
        self.resource.thumbnail(representation_id).await
    }

    pub async fn read_entry_file_resource(
        &self,
        entry_id: &str,
    ) -> Result<crate::facade::FileResourceView, crate::facade::ResourceFacadeError> {
        self.resource.entry_file(entry_id).await
    }

    /// A1:初始化空间。外部业务入口从 `AppFacade` 进入,不直接拿 `SpaceFacade`。
    pub async fn initialize_space(
        &self,
        input: InitializeSpaceInput,
    ) -> Result<InitializeSpaceResult, InitializeSpaceError> {
        self.space_session.initialize_space(input).await
    }

    /// A2: unlock a space through the top-level application facade.
    pub async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError> {
        self.space_session.unlock_space(input).await
    }

    pub async fn recover_space_session(
        &self,
        allow_secure_storage_unlock: bool,
    ) -> Result<RecoverSpaceSessionResult, SpaceSessionError> {
        self.space_session
            .recover_session(allow_secure_storage_unlock)
            .await
    }

    pub async fn lock_space_session(&self) -> Result<(), SpaceSessionError> {
        self.space_session.lock_space().await
    }

    pub async fn join_space(
        &self,
        input: crate::facade::JoinSpaceInput,
    ) -> Result<crate::facade::JoinSpaceResult, crate::facade::JoinSpaceError> {
        let result = self.space_admission.join_space(input).await?;
        match &result {
            crate::facade::JoinSpaceResult::Fresh(_) => {
                self.space_session
                    .recover_session(true)
                    .await
                    .map_err(|error| crate::facade::JoinSpaceError::Activity(error.to_string()))?;
            }
            crate::facade::JoinSpaceResult::Switched(_) => {
                self.space_session
                    .resume_after_space_change()
                    .await
                    .map_err(|error| crate::facade::JoinSpaceError::Activity(error.to_string()))?;
            }
        }
        Ok(result)
    }

    /// Read setup state through the top-level application facade.
    pub async fn query_setup_state(&self) -> Result<SetupStateView, QuerySetupStateError> {
        self.space.query_setup_state().await
    }

    pub async fn factory_reset_space(&self) -> Result<(), crate::facade::FactoryResetError> {
        self.space_session.factory_reset().await
    }

    pub async fn reset_space(&self) -> Result<(), crate::facade::ResetSpaceError> {
        self.space.reset().await
    }

    /// 尝试静默恢复空间会话。
    pub async fn try_resume_session(&self) -> Result<bool, TryResumeSessionError> {
        self.space.try_resume_session().await
    }

    /// 刷新成员在线状态。
    pub async fn refresh_presence(
        &self,
    ) -> Result<EnsureReachableAllReport, EnsureReachableAllError> {
        self.space.refresh_presence().await
    }

    pub async fn recover_network(&self) -> Result<(), NetworkRecoveryRequestError> {
        self.network_recovery.request_recovery().await
    }

    pub async fn network_recovery_status(&self) -> NetworkRecoveryStatus {
        self.network_recovery.status().await
    }

    /// 列出已配对 peer 的 `DeviceId`(本机已过滤)。供 desktop keepalive
    /// 调度器用来发现新 peer / 收回已删除 peer。Thin wrapper over
    /// [`SpaceFacade::list_paired_peer_device_ids`].
    pub async fn list_paired_peer_device_ids(
        &self,
    ) -> Result<Vec<DeviceId>, EnsureReachableAllError> {
        self.space.list_paired_peer_device_ids().await
    }

    /// 对单个 peer 触发一次 `ensure_reachable`。供 desktop keepalive 调度
    /// 器在退避到期时按需拨号。Thin wrapper over
    /// [`SpaceFacade::ensure_reachable_one`].
    pub async fn ensure_reachable_one(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PresenceError> {
        self.space.ensure_reachable_one(device).await
    }

    /// B1:签发配对邀请。
    pub async fn issue_pairing_invitation(
        &self,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.space_admission.issue_invitation().await
    }

    /// 按指定本机地址签发配对邀请。
    pub async fn issue_pairing_invitation_for_address(
        &self,
        selected_ip: IpAddr,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.space_admission
            .issue_invitation_for_address(selected_ip)
            .await
    }

    /// 列出当前可用于配对邀请的本机地址。
    pub async fn list_pairing_invitation_addresses(
        &self,
    ) -> Result<Vec<PairingInvitationAddressCandidate>, IssuePairingInvitationError> {
        self.space_admission.list_invitation_addresses().await
    }

    /// B2:兑换配对邀请。
    pub async fn redeem_pairing_invitation(
        &self,
        input: RedeemPairingInvitationInput,
    ) -> Result<RedeemPairingInvitationResult, RedeemPairingInvitationError> {
        let result = self.space_admission.redeem_invitation(input).await?;
        self.space_session
            .recover_session(true)
            .await
            .map_err(|error| {
                RedeemPairingInvitationError::Internal(format!("activate paired space: {error}"))
            })?;
        Ok(result)
    }

    /// 已 setup 设备加入另一个 sponsor 空间，4 阶段重加密迁移。详见
    /// `usecases::setup::switch_space` 模块文档。
    pub async fn switch_space(
        &self,
        input: SwitchSpaceInput,
    ) -> Result<SwitchSpaceResult, SwitchSpaceError> {
        self.space_admission.switch_space(input).await
    }

    /// 查询当前 switch-space 迁移进度（粗粒度——只返回阶段和备份表条目数）。
    pub async fn query_migration_progress(
        &self,
    ) -> Result<MigrationProgress, QueryMigrationProgressError> {
        self.space.query_migration_progress().await
    }

    /// 订阅配对完成事件。
    pub fn subscribe_pairing_completion(
        &self,
    ) -> Result<broadcast::Receiver<PairingOutcome>, IssuePairingInvitationError> {
        Ok(self.space_admission.subscribe_pairing_completion())
    }

    pub async fn cancel_invitation(&self) -> Result<(), crate::facade::CancelInvitationError> {
        self.space_admission.cancel_invitation().await
    }

    /// 列出对外成员摘要。外部调用只经过 `AppFacade`,不直接依赖 roster 子 facade。
    pub async fn list_members(&self) -> Result<Vec<MemberSummary>, RosterError> {
        self.member_roster.list_members().await
    }

    /// 列出带 presence 的 roster entry。
    pub async fn list_roster_entries(
        &self,
    ) -> Result<Vec<crate::facade::roster::RosterEntry>, RosterError> {
        self.member_roster.list_with_presence().await
    }

    /// 发送一个剪贴板快照到在线 peer。
    ///
    /// `target_filter`:
    /// - `None` —— 全 fan-out（向所有 trusted online peer）;
    /// - `Some(list)` —— 仅向指定 device 集合 fan-out;空列表合法,表示零目标。
    ///
    /// 不绕过 `is_send_allowed` / member gating / presence 这三层 use case
    /// 内部检查,filter 在它们之后生效。
    pub async fn dispatch_clipboard_snapshot(
        &self,
        snapshot: SystemClipboardSnapshot,
        origin: ClipboardChangeOrigin,
        target_filter: Option<Vec<DeviceId>>,
    ) -> Result<crate::facade::DispatchEntryOutcome, ClipboardSyncError> {
        self.clipboard_sync
            // CLI / 直接调用方不与某条 entry 绑定,跳过 delivery 落盘;
            // target_filter 透传到下层 dispatch_entry。
            .dispatch_snapshot(snapshot, origin, None, target_filter)
            .await
    }

    /// 取一条 entry 的"来源 + 每个对端同步状态"完整视图。GUI detail
    /// 面板用它渲染"来自哪台设备 / 同步到了哪些设备 / 哪台失败"。
    pub async fn get_entry_delivery_view(
        &self,
        entry_id: &uc_core::ids::EntryId,
    ) -> Result<crate::facade::EntryDeliveryView, crate::facade::GetEntryDeliveryViewError> {
        self.clipboard_sync.get_entry_delivery_view(entry_id).await
    }

    pub async fn get_entry_receive_progress(
        &self,
        entry_id: &uc_core::ids::EntryId,
    ) -> Result<Option<uc_core::ports::EntryReceiveProgress>, crate::facade::CancelEntryReceiveError>
    {
        self.clipboard_sync
            .get_entry_receive_progress(entry_id)
            .await
    }

    pub async fn list_entry_receive_progress(
        &self,
    ) -> Result<Vec<uc_core::ports::EntryReceiveProgress>, crate::facade::CancelEntryReceiveError>
    {
        self.clipboard_sync.list_entry_receive_progress().await
    }

    pub async fn cancel_entry_receive(
        &self,
        entry_id: &uc_core::ids::EntryId,
        expected_attempt_id: &str,
    ) -> Result<crate::facade::CancelEntryReceiveOutcome, crate::facade::CancelEntryReceiveError>
    {
        self.clipboard_sync
            .cancel_entry_receive(entry_id, expected_attempt_id)
            .await
    }

    /// 用户主动 resend 一条本机来源的 entry。GUI / Tauri command / CLI
    /// `uniclip send --resend` 都从这里进。详细语义见
    /// [`ClipboardOutboundFacade::resend_entry`]。
    ///
    pub async fn resend_entry(
        &self,
        cmd: ResendEntryCommand,
    ) -> Result<ResendReport, ResendEntryError> {
        self.clipboard_outbound.resend_entry(cmd).await
    }

    /// 发布 blob。
    pub async fn publish_blob(
        &self,
        command: PublishBlobCommand,
    ) -> Result<PublishBlobResult, BlobTransferError> {
        self.blob_transfer.publish_blob(command).await
    }

    /// 拉取 blob。
    pub async fn fetch_blob(
        &self,
        command: FetchBlobCommand,
    ) -> Result<FetchBlobResult, BlobTransferError> {
        self.blob_transfer.fetch_blob(command).await
    }

    /// 流式 publish 一个磁盘文件作为 blob。
    ///
    /// 内存峰值与文件大小解耦(走 iroh-blobs `add_path` + reflink_or_copy);
    /// 适合 CLI / GUI 的 user-facing 大文件发送入口。
    pub async fn publish_blob_path(
        &self,
        command: PublishBlobPathCommand,
    ) -> Result<PublishBlobResult, BlobTransferError> {
        self.blob_transfer.publish_blob_path(command).await
    }

    /// 流式 fetch 一个 blob 到指定本地文件。
    ///
    /// 与 [`Self::fetch_blob`] 的差别:bytes 落在 `target_path`,不返回内存。
    /// 当 `command.transfer_context` 提供时,fetch 会被注册到 inflight
    /// registry 上;之后调 [`Self::cancel_inbound_transfer`] 可以中断它。
    pub async fn fetch_blob_to_path(
        &self,
        command: FetchBlobToPathCommand,
    ) -> Result<FetchBlobToPathResult, BlobTransferError> {
        self.blob_transfer.fetch_blob_to_path(command).await
    }

    /// 把一个剪贴板快照连同已 publish 的 blob 引用一起 dispatch。
    ///
    /// 与 [`Self::dispatch_clipboard_snapshot`] 区别:本方法适用于 sender
    /// 已经把文件 publish 成 blob 的场景,blob_refs 会被编码进 V3 envelope
    /// 尾部扩展,接收端 inbound materializer 通过 ticket 拉取。
    pub async fn dispatch_clipboard_snapshot_with_blob_refs(
        &self,
        snapshot: SystemClipboardSnapshot,
        blob_refs: Vec<V3BlobRef>,
        origin: ClipboardChangeOrigin,
    ) -> Result<DispatchEntryOutcome, ClipboardSyncError> {
        self.clipboard_sync
            .dispatch_snapshot_with_blob_refs(snapshot, blob_refs, origin, None, None)
            .await
    }

    /// 取消一次进行中的 inbound 文件传输。
    ///
    /// 接收方主动撤回 fetch:trigger 内部 cancellation token + 撕掉
    /// iroh-blobs Downloader 用的 QUIC connection + 落 `Cancelled`
    /// domain event。幂等:同一 `transfer_id` 不在 inflight registry
    /// 时(没有进行中的 fetch / 已经被取消过)返回 `Ok(NotInflight)`,
    /// 实际撤回则返回 `Ok(Cancelled)` —— timeout sweep / 删除流程靠这个
    /// 区分来决定是否要走 fallback 终结(例如 `mark_failed` pending 行)。
    pub async fn cancel_inbound_transfer(
        &self,
        transfer_id: &str,
        reason: uc_core::FileTransferCancellationReason,
    ) -> Result<crate::facade::InboundCancelOutcome, BlobTransferError> {
        self.blob_transfer
            .cancel_inbound_transfer(transfer_id, reason)
            .await
    }

    /// 查询本地搜索索引。
    pub async fn search_query(
        &self,
        input: SearchQueryInput,
    ) -> Result<SearchPageView, SearchFacadeError> {
        self.search.query(input).await
    }

    pub async fn search_tags(
        &self,
    ) -> Result<Vec<crate::facade::SearchTagView>, SearchFacadeError> {
        self.search.tags().await
    }

    /// 查询本地搜索状态。
    pub async fn search_status(&self) -> Result<SearchStatusView, SearchFacadeError> {
        self.search.status().await
    }

    pub async fn request_search_rebuild(
        &self,
    ) -> Result<SearchRebuildAcceptedView, SearchFacadeError> {
        self.search.request_rebuild().await
    }

    /// 查询加密/初始化状态。
    pub async fn encryption_state(&self) -> Result<EncryptionStateView, EncryptionFacadeError> {
        self.encryption.state().await
    }

    pub async fn verify_secure_storage_access(&self) -> Result<bool, EncryptionFacadeError> {
        self.encryption.verify_keychain_access().await
    }

    pub async fn local_device_info(
        &self,
    ) -> Result<crate::facade::LocalDeviceInfoView, crate::facade::DeviceFacadeError> {
        self.device.local_device_info().await
    }

    pub async fn settings(&self) -> Result<crate::facade::SettingsView, SettingsFacadeError> {
        self.settings.get().await
    }

    pub async fn update_settings(
        &self,
        patch: SettingsPatch,
    ) -> Result<crate::facade::SettingsView, SettingsFacadeError> {
        self.settings.update(patch).await
    }

    pub async fn probe_relay_url(
        &self,
        url: &str,
        credential: crate::facade::settings::RelayProbeCredential,
    ) -> Result<crate::facade::RelayProbeReportView, SettingsFacadeError> {
        self.settings.probe_relay_url(url, credential).await
    }

    pub async fn save_relay(
        &self,
        patch: SettingsPatch,
        edit: crate::facade::settings::RelayCredentialEdit,
    ) -> Result<crate::facade::settings::RelaySaveView, SettingsFacadeError> {
        self.settings.save_relay(patch, edit).await
    }

    pub fn relay_credential_status(
        &self,
        url: &str,
    ) -> Result<crate::facade::settings::RelayCredentialStatusView, SettingsFacadeError> {
        self.settings.relay_credential_status(url)
    }

    pub async fn export_config(
        &self,
        destination: &Path,
    ) -> Result<PathBuf, uc_core::ports::config_migration::ConfigMigrationError> {
        self.config_migration.export_config(destination).await
    }

    pub async fn preview_config_import(
        &self,
        password: &uc_core::crypto::domain::Passphrase,
        source: &Path,
    ) -> Result<
        uc_core::ports::config_migration::ConfigImportPreview,
        uc_core::ports::config_migration::ConfigMigrationError,
    > {
        self.config_migration.preview_import(password, source).await
    }

    pub async fn stage_config_import(
        &self,
        password: &uc_core::crypto::domain::Passphrase,
        source: &Path,
    ) -> Result<
        uc_core::ports::config_migration::StagedConfigImport,
        uc_core::ports::config_migration::ConfigMigrationError,
    > {
        self.config_migration.stage_import(password, source).await
    }

    pub async fn member_sync_preferences(
        &self,
        device_id: &str,
    ) -> Result<crate::facade::MemberSyncPreferencesView, RosterError> {
        self.member_roster.get_sync_preferences(device_id).await
    }

    pub async fn update_member_sync_preferences(
        &self,
        device_id: &str,
        patch: crate::facade::MemberSyncPreferencesPatch,
    ) -> Result<crate::facade::MemberSyncPreferencesView, RosterError> {
        self.member_roster
            .update_sync_preferences(device_id, patch)
            .await
    }

    pub async fn remove_member(
        &self,
        device_id: &str,
    ) -> Result<crate::facade::WorkspaceSnapshot, RosterError> {
        self.member_roster.submit_member_removal(device_id).await
    }

    pub async fn workspace_convergence(
        &self,
    ) -> Result<crate::facade::WorkspaceSnapshot, RosterError> {
        self.member_roster.query_workspace_convergence().await
    }

    pub fn subscribe_workspace_convergence(
        &self,
    ) -> broadcast::Receiver<crate::facade::WorkspaceSnapshot> {
        self.member_roster.subscribe_workspace_convergence()
    }

    pub async fn space_protection(
        &self,
    ) -> Result<crate::facade::SpaceProtectionView, RosterError> {
        self.member_roster.query_space_protection().await
    }

    pub async fn legacy_bootstrap(
        &self,
        bootstrap_id: &str,
    ) -> Result<Option<crate::facade::LegacyBootstrapView>, RosterError> {
        self.member_roster
            .query_legacy_bootstrap(bootstrap_id)
            .await
    }

    pub async fn diagnostics_status(
        &self,
    ) -> Result<crate::facade::DebugStatusView, crate::facade::DiagnosticsFacadeError> {
        self.diagnostics.debug_status().await
    }

    pub async fn update_debug_mode(
        &self,
        enabled: bool,
    ) -> Result<crate::facade::UpdateDebugModeView, crate::facade::DiagnosticsFacadeError> {
        self.diagnostics.set_debug_mode(enabled).await
    }

    pub async fn export_diagnostic_logs(
        &self,
        since_hours: Option<u32>,
        destination: PathBuf,
    ) -> Result<crate::facade::LogExportView, crate::facade::DiagnosticsFacadeError> {
        self.diagnostics
            .export_logs_to_dir(since_hours, destination)
            .await
    }

    pub async fn storage_stats(
        &self,
    ) -> Result<crate::facade::StorageStatsView, crate::facade::StorageFacadeError> {
        self.storage.stats().await
    }

    pub async fn clear_storage_cache(
        &self,
    ) -> Result<crate::facade::ClearCacheResultView, crate::facade::StorageFacadeError> {
        self.storage.clear_cache().await
    }

    pub async fn upgrade_status(
        &self,
        current_version: &str,
    ) -> Result<crate::facade::UpgradeStatus, crate::facade::DetectUpgradeError> {
        self.upgrade.detect_on_startup(current_version).await
    }

    pub async fn acknowledge_upgrade(
        &self,
        current_version: &str,
    ) -> Result<(), crate::facade::AcknowledgeUpgradeError> {
        self.upgrade.acknowledge(current_version).await
    }

    /// 更新本机设备名。
    pub async fn set_device_name(&self, device_name: String) -> Result<(), SettingsFacadeError> {
        let current = self.settings.get().await?;
        if current.general.device_name.as_deref() == Some(device_name.as_str()) {
            return Ok(());
        }

        self.settings
            .update(SettingsPatch {
                general: Some(GeneralSettingsPatch {
                    device_name: Some(Some(device_name)),
                    auto_start: None,
                    startup_mode: None,
                    restore_last_entry_on_startup: None,
                    auto_check_update: None,
                    auto_download_update: None,
                    theme: None,
                    theme_color: None,
                    theme_color_light: None,
                    theme_color_dark: None,
                    theme_overrides_light: None,
                    theme_overrides_dark: None,
                    language: None,
                    update_channel: None,
                    telemetry_enabled: None,
                    usage_analytics_enabled: None,
                    debug_mode: None,
                }),
                sync: None,
                retention_policy: None,
                security: None,
                pairing: None,
                keyboard_shortcuts: None,
                file_sync: None,
                network: None,
                quick_panel: None,
            })
            .await?;
        Ok(())
    }

    /// 列出对外 peer 快照。外部调用只经过 `AppFacade`,不直接依赖 roster 子 facade。
    pub async fn list_peer_snapshots(&self) -> Result<Vec<PeerSnapshotView>, RosterError> {
        self.member_roster.list_peer_snapshots().await
    }

    /// 订阅成员在线状态变化。外部拿到的是 application 事件,不暴露 core 事件类型。
    pub fn subscribe_peer_presence_events(&self) -> Result<AppPresenceSubscription, RosterError> {
        let inner = self.member_roster.subscribe_presence_events();
        Ok(AppPresenceSubscription { inner })
    }
}

/// application 层 presence 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPresenceEvent {
    pub device_id: String,
    pub state: String,
    pub at_ms: i64,
}

/// application 层 presence 订阅错误。
#[derive(Debug, Error)]
pub enum AppPresenceSubscriptionError {
    #[error("presence event receiver lagged by {0} messages")]
    Lagged(u64),
    #[error("presence event receiver closed")]
    Closed,
}

/// application 层 presence 订阅句柄。
pub struct AppPresenceSubscription {
    inner: broadcast::Receiver<PresenceEvent>,
}

impl AppPresenceSubscription {
    pub async fn recv(&mut self) -> Result<AppPresenceEvent, AppPresenceSubscriptionError> {
        self.inner
            .recv()
            .await
            .map(presence_event_to_app)
            .map_err(|err| match err {
                broadcast::error::RecvError::Lagged(skipped) => {
                    AppPresenceSubscriptionError::Lagged(skipped)
                }
                broadcast::error::RecvError::Closed => AppPresenceSubscriptionError::Closed,
            })
    }
}

fn presence_event_to_app(event: PresenceEvent) -> AppPresenceEvent {
    AppPresenceEvent {
        device_id: event.device_id.as_str().to_string(),
        state: reachability_state_to_string(event.state),
        at_ms: event.at.timestamp_millis(),
    }
}

fn reachability_state_to_string(state: ReachabilityState) -> String {
    match state {
        ReachabilityState::Online => "online",
        ReachabilityState::Offline => "offline",
        ReachabilityState::Unknown => "unknown",
    }
    .to_string()
}

pub struct AppFacadeParts {
    pub space: Arc<SpaceFacade>,
    pub space_session_activity: SpaceSessionActivityDeps,
    pub space_session_access: SpaceSessionAccessDeps,
    pub space_application: SpaceApplicationHandle,
    pub member_roster: Arc<MemberRosterFacade>,
    pub encryption: Arc<EncryptionFacade>,
    pub resource: Arc<ResourceFacade>,
    pub clipboard_history: Arc<ClipboardHistoryFacade>,
    pub clipboard_capture: Arc<ClipboardCaptureFacade>,
    pub clipboard_sync: Arc<ClipboardSyncFacade>,
    pub blob_transfer: Arc<BlobTransferFacade>,
    pub clipboard_outbound: Arc<ClipboardOutboundFacade>,
    pub clipboard_restore: Arc<ClipboardRestoreFacade>,
    pub search: Arc<SearchFacade>,
    pub settings: Arc<SettingsFacade>,
    pub diagnostics: Arc<DiagnosticsFacade>,
    pub device: Arc<DeviceFacade>,
    pub storage: Arc<StorageFacade>,
    pub config_migration: Arc<ConfigMigrationFacade>,
    pub upgrade: Arc<UpgradeFacade>,
    pub network_recovery: Arc<NetworkRecoveryFacade>,
}
