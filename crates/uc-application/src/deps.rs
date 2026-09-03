//! # Application 依赖
//!
//! 定义 Application 对象图所需的依赖分组。这些类型只负责打包必需参数，
//! 不提供分步构建、默认值或隐式装配逻辑，因此不是 Builder。

use std::sync::Arc;
use tokio::sync::mpsc;
use uc_core::blob::ports::{BlobContentIngestPort, BlobReaderPort, BlobWriterPort};
use uc_core::ids::RepresentationId;
use uc_core::membership::GroupRevocationPort;
use uc_core::ports::clipboard::{
    AdvanceActiveClipboardPort, CheckEntryAvailabilityPort, ClipboardPayloadResolverPort,
    ClipboardRepresentationNormalizerPort, DeleteClipboardEntryPort, EntryFileSetRepositoryPort,
    FindEntryIdBySnapshotHashPort, GetClipboardEntryPort, GetEntrySnapshotHashPort,
    GetRepresentationByBlobIdPort, GetRepresentationPort, ListClipboardEntriesPort,
    ListRepresentationsForEventPort, LoadActiveClipboardPort, LoadMobileConsumableClipboardPort,
    ReplaceEntryContentPort, RepresentationCachePort, ResetActiveClipboardPort,
    SaveClipboardEntryPort, SelfWriteLedgerPort, SetClipboardEntryFavoritePort, SpoolQueuePort,
    SystemClipboardPort, ThumbnailGeneratorPort, ThumbnailRepositoryPort, TouchClipboardEntryPort,
    UpdateRepresentationProcessingResultPort,
};
use uc_core::ports::search::maintenance::SearchIndexMaintenancePort;
use uc_core::ports::search::search_index::SearchIndexPort;
use uc_core::ports::search::search_key::SearchKeyDerivationPort;
use uc_core::ports::search::search_pipeline::SearchPipelinePort;
use uc_core::ports::space::DeriveSpaceSubkeyPort;
use uc_core::ports::*;
use uc_core::MemberRepositoryPort;
use uc_observability_contract::analytics::AnalyticsPort;

pub use crate::application::{
    ApplicationAdapters, ApplicationClipboardAdapters, ApplicationHostAdapters,
    ApplicationNetworkAdapters, ApplicationNetworkBinding, ApplicationSpaceAdapters,
};
pub use crate::clipboard::assembly::{
    ClipboardBackgroundError, ClipboardBackgroundPort, ClipboardBackgroundStartError,
};
use crate::clipboard::write::{MobileConsumabilityProbe, MobileConsumableBackfill};
pub use crate::facade::config_migration::ConfigMigrationDeps;
pub use crate::profile::factory_reset::{
    ClearProfileStatePort, FactoryResetPhase, PrepareProfileLifecycleUseCase,
    ProfileFactoryResetCapabilityError, ProfileGeneration, ProfileLifecycle, ProfileLifecycleError,
    ProfileLifecycleRepositoryError, ProfileLifecycleRepositoryPort, ProfileLifecycleState,
    StopProfileRuntimePort, WipeProfileKeysPort,
};
pub use crate::profile::probe_profile_key_access::{
    ProbeProfileKeyAccessPort, ProbeProfileKeyAccessUseCase, ProfileKeyAccessProbe,
    ProfileKeyAccessProbePortError,
};
use crate::search::mutation_gate::{CoordinatedSearchIndex, SearchMutationGate};
pub use crate::space::SpaceRuntimeAdapters;
pub use crate::space::{
    ActivateCompletionHelperAdmissionSecurityPort,
    ActivateCompletionHelperAdmissionSecurityRequest, ActivateMembershipEffectPort,
    ActivateSponsorAdmissionError, ActivateSponsorAdmissionPort,
    ActivateSponsorAdmissionSecurityPort, ActivateSponsorAdmissionSecurityRequest,
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AdmissionSecurityTransitionError, AdmissionSecurityTransitionInput,
    AdmissionSecurityTransitionPort, AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission, CompletedJoinerActivation,
    CurrentJoinAdmissionStatePort, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
    HandleAuthenticatedSpaceAdmissionMessageError, HandleAuthenticatedSpaceAdmissionMessagePort,
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationOutcome,
    JoinerActivationStateError, JoinerActivationStatePort, JoinerCancellationCommitToken,
    JoinerCancellationMaterial, JoinerCancellationMaterialError, JoinerCancellationMutation,
    JoinerCancellationStateError, JoinerStartMaterial, JoinerStartMaterialError,
    JoinerStartMaterialPort, JoinerStartMutation, JoinerStartStateError, JoinerStartStatePort,
    LoadedCurrentJoin, LoadedJoinerActivation, LoadedJoinerStartState, LoadedPendingAdmission,
    LoadedSponsorAdmission, PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PrepareJoinerAppliedError,
    PrepareJoinerAppliedPort, PrepareJoinerCancellationPort, PrepareJoinerCandidateError,
    PrepareJoinerCandidatePort, PrepareJoinerInvitationError, PrepareJoinerInvitationPort,
    PrepareSponsorAdmissionSecurityPort, PrepareSponsorCandidateError, PrepareSponsorCandidatePort,
    PrepareSponsorCommitError, PrepareSponsorCommitPort, PrepareSponsorCompleteError,
    PrepareSponsorCompletePort, PrepareSponsorSettledError, PrepareSponsorSettledPort,
    PreparedJoinerActivation, PreparedJoinerAppliedMaterial, PreparedJoinerCandidateMaterial,
    PreparedJoinerInvitation, PreparedMemberSecurityDelivery, PreparedSponsorCandidate,
    PreparedSponsorCommit, PreparedSponsorComplete, PreparedSponsorSettled, RePairingStateError,
    RePairingStateStorePort, RebindSpaceSessionPort, RecoverMembershipEffectsPort,
    RecoverSpaceAdmissionsPort, ResolveJoinerInvitationError, ResolveJoinerInvitationPort,
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, ResumeSpaceSessionPort, SpaceActivityError,
    SpaceAdmissionCommitToken, SpaceAdmissionMessageReply, SpaceAdmissionTransportError,
    SpaceAdmissionTransportPort, SpaceMemberPauseReason, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionSecurityRecipient, SponsorAdmissionSecurityRequest,
    SponsorAdmissionState, SponsorAdmissionStateError, SponsorAdmissionStatePort,
    SponsorPreparedAdmissionSecurity, SponsorPreparedSecurityTransition,
};
pub use crate::space::{
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort, BeginMembershipBranchRecoveryInput,
    CleanupLegacyMembershipDataPort, CommitMembershipLedgerPort, CurrentMemberSignatureError,
    CurrentMemberSignaturePort, CurrentSpaceIdentityError, CurrentSpaceIdentityPort,
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    DeliverRestrictedMembershipPort, DeviceManagementResetDataPort, InboundMembershipTransfer,
    InitialSpaceActivationPort, InitializeSpacePort, InitiatedMembershipRemovalEffect,
    IsSpaceUnlockedPort, IssueMembershipBranchRecoveryError, IssueMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryPort, JoinerStagedSecurityTransition, LoadCurrentJoinStatusPort,
    LoadDeviceTrustObservationsPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    LockSpacePort, MembershipBranchRecoveryChannelError, MembershipBranchRecoveryChannelPort,
    MembershipBranchRecoveryCommit, MembershipBranchRecoveryRequest,
    MembershipBranchRecoverySession, MembershipBranchRecoverySessionState,
    MembershipEffectExecutionError, MembershipEffectKind, MembershipEffectPhase,
    MembershipLedgerError, MembershipLedgerMutation, MembershipMaintenanceStepOutcome,
    MembershipNetworkActivityPort, PausedSpaceMember, PeerHistorySyncState,
    PeerReconciliationRecord, PendingMembershipEffect, PortableCurrentSpaceIdentityPort,
    PrepareMembershipBranchRecoveryMaterialError, PrepareMembershipBranchRecoveryMaterialInput,
    PrepareMembershipBranchRecoveryMaterialPort, PrepareMembershipBranchRecoveryRecipientError,
    PrepareMembershipBranchRecoveryRecipientPort, PrepareMembershipBranchTransitionError,
    PrepareMembershipBranchTransitionInput, PrepareMembershipBranchTransitionPort,
    PrepareSpaceAdmissionCredentialsPort, PreparedMembershipBranchRecoveryMaterial,
    PreparedMembershipBranchRecoveryRecipient, QueryDeviceTrustError,
    SpaceAdmissionCredentialPreparationError, SpaceRebuildProgressError, SpaceRebuildProgressPort,
    SpaceSessionRebindError, UnlockSpacePort,
};
pub use crate::transfer::file::assembly::ReceiveCancellationDeps;
pub use crate::transfer::file::facade::FileTransferFacadeDeps;
pub use crate::transfer::file::lifecycle::FileTransferLifecycleDeps;
pub use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;

#[cfg(feature = "test-support")]
pub mod test_support {
    pub use crate::clipboard::sync::apply_inbound::ApplyInboundClipboardUseCase;
}

/// 剪贴板条目的意图端口集合。
///
/// 组合根将同一个 Diesel 条目适配器投影为这些窄端口，消费者只声明
/// 实际调用的能力。
#[derive(Clone)]
pub struct ClipboardEntryPorts {
    /// 按条目 ID 读取单个剪贴板条目。
    pub get: Arc<dyn GetClipboardEntryPort>,
    /// 以倒序分页方式列出剪贴板条目。
    pub list: Arc<dyn ListClipboardEntriesPort>,
    /// 原子保存条目及其表示选择决策。
    pub save: Arc<dyn SaveClipboardEntryPort>,
    /// 更新已有条目的最后活跃时间。
    pub touch: Arc<dyn TouchClipboardEntryPort>,
    /// 更新已有条目的收藏状态。
    pub set_favorite: Arc<dyn SetClipboardEntryFavoritePort>,
    /// 以幂等方式删除剪贴板条目。
    pub delete: Arc<dyn DeleteClipboardEntryPort>,
    /// 在同一事务中删除条目及其接收状态。
    pub delete_with_receive_state: Arc<dyn DeleteClipboardEntryWithReceiveStatePort>,
    /// 按已持久化的跨设备快照哈希反查条目 ID。
    pub find_by_snapshot_hash: Arc<dyn FindEntryIdBySnapshotHashPort>,
    /// 按条目 ID 查询已持久化的跨设备快照哈希。
    ///
    /// 恢复流程必须读取该值，不得从重建快照重新计算，否则文件条目的
    /// 结果可能与原始内容身份不同。
    pub get_snapshot_hash: Arc<dyn GetEntrySnapshotHashPort>,
    /// 结合数据库表示状态与文件系统实时检查条目是否完整可用。
    ///
    /// 入站去重依此区分“已完整持有”与“仅部分持有、需就地升级”。
    pub availability: Arc<dyn CheckEntryAvailabilityPort>,
    /// 以事务方式就地替换条目内容，复用条目 ID 并保留粘性用户状态。
    ///
    /// 该端口负责把入站的部分条目升级为完整条目。
    pub replace_content: Arc<dyn ReplaceEntryContentPort>,
}

/// 面向 Application 层的剪贴板表示意图端口集合。
///
/// 组合根将解密 decorator 投影为这些窄端口。后台 payload worker 保留更完整的
/// 内部存储接口，Application 层只依赖读取与处理结果更新能力。
#[derive(Clone)]
pub struct ClipboardRepresentationPorts {
    /// 在所属事件上下文中读取指定表示。
    pub get: Arc<dyn GetRepresentationPort>,
    /// 按 Blob ID 读取引用该 Blob 的表示。
    pub get_by_blob_id: Arc<dyn GetRepresentationByBlobIdPort>,
    /// 列出指定剪贴板事件的全部表示。
    pub list_for_event: Arc<dyn ListRepresentationsForEventPort>,
    /// 以比较并更新方式原子推进表示的处理结果。
    pub update_processing_result: Arc<dyn UpdateRepresentationProcessingResultPort>,
}

/// 剪贴板领域端口组。
#[derive(Clone)]
pub struct ClipboardPorts {
    /// 读取平台剪贴板快照的端口。
    pub clipboard: Arc<dyn PlatformClipboardPort>,
    /// 读取或写入操作系统剪贴板快照的完整端口。
    pub system_clipboard: Arc<dyn SystemClipboardPort>,
    /// 剪贴板条目的窄意图端口集合。
    pub entry_ports: ClipboardEntryPorts,
    /// 持久化剪贴板事件的写端口。
    pub clipboard_event_repo: Arc<dyn ClipboardEventWriterPort>,
    /// 与 `clipboard_event_repo` 共用存储的剪贴板事件读端口。
    ///
    /// 它提供事件来源设备等只读查询，用于填充搜索索引的 `source_device`
    /// 渲染列。
    pub clipboard_event_reader_repo: Arc<dyn ClipboardEventRepositoryPort>,
    /// 向后台 payload worker 提供完整聚合能力的内部表示存储。
    ///
    /// 普通 Application 流程应依赖 `representation_ports`，不应直接使用此宽接口。
    pub representation_store: Arc<dyn ClipboardRepresentationStore>,
    /// 面向 Application 层的剪贴板表示窄端口集合。
    pub representation_ports: ClipboardRepresentationPorts,
    /// 将平台表示规范化为稳定领域表示的端口。
    pub representation_normalizer: Arc<dyn ClipboardRepresentationNormalizerPort>,
    /// 读取或删除剪贴板表示选择决策的仓储端口。
    pub selection_repo: Arc<dyn ClipboardSelectionRepositoryPort>,
    /// 从候选表示中选择首选内容的策略端口。
    pub representation_policy: Arc<dyn SelectRepresentationPolicyPort>,
    /// 管理表示本地缓存的端口。
    pub representation_cache: Arc<dyn RepresentationCachePort>,
    /// 将包含 payload 字节的表示任务加入 spool 队列的端口。
    pub spool_queue: Arc<dyn SpoolQueuePort>,
    /// 记录本机写入以识别剪贴板变化来源的端口。
    pub clipboard_change_origin: Arc<dyn SelfWriteLedgerPort>,
    /// 向后台 payload worker 投递待处理表示 ID 的通道。
    pub worker_tx: mpsc::Sender<RepresentationId>,
    /// 将表示解析为可消费 payload 的端口。
    pub payload_resolver: Arc<dyn ClipboardPayloadResolverPort>,
    /// 推进跨设备活动剪贴板 LWW 寄存器的写端口。
    ///
    /// 恢复或入站应用流程把内容写入本机系统剪贴板后，通过它将该内容设为
    /// 最新活动状态。
    pub active_register: Arc<dyn AdvanceActiveClipboardPort>,
    /// 读取跨设备活动剪贴板 LWW 寄存器的端口。
    ///
    /// 入站 `0xC3` 状态处理器依此判断新观测是否取代当前值，并阻断回环。
    pub active_register_load: Arc<dyn LoadActiveClipboardPort>,
    /// 读取最新可供移动端消费的活动剪贴板内容引用。
    pub mobile_consumable_load: Arc<dyn LoadMobileConsumableClipboardPort>,
    /// 为旧活动剪贴板记录回填移动端可消费状态。
    pub mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    /// 判断条目内容能否发送到移动端的共享探针。
    ///
    /// 目录形态的文件集不可供移动端消费。组合根只构造一次该探针，各寄存器
    /// 推进路径直接复用，不再从原始仓储重复装配。
    pub mobile_consumability: MobileConsumabilityProbe,
    /// 无条件清空跨设备活动剪贴板 LWW 寄存器的端口。
    ///
    /// 启动核对使用它删除已与实时系统剪贴板不一致的持久记录。
    pub active_register_reset: Arc<dyn ResetActiveClipboardPort>,
}

/// 面向 Application 层的 Space 访问意图端口集合。
///
/// 组合根将同一个 Space 访问适配器投影为这些窄端口。依赖分组可以携带
/// 整个集合，但每个用例只接收它实际需要的能力。
#[derive(Clone)]
pub struct SpaceAccessPorts {
    /// 将安全会话重绑定到指定的孤立 Space。
    pub adopt_isolated_space: Arc<dyn RebindSpaceSessionPort>,
    /// 使用口令初始化 Space 并建立活动会话。
    pub initialize: Arc<dyn InitializeSpacePort>,
    /// 使用口令解锁 Space 并建立活动会话。
    pub unlock: Arc<dyn UnlockSpacePort>,
    /// 查询指定 Space 当前是否已解锁。
    pub is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
    /// 锁定指定 Space 并结束其活动会话。
    pub lock: Arc<dyn LockSpacePort>,
    /// 尝试从已保存的密钥材料恢复 Space 会话。
    pub resume_session: Arc<dyn ResumeSpaceSessionPort>,
    /// 从当前已解锁会话派生用途隔离的子密钥。
    pub derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
    /// 在不切换当前 Space 的前提下准备准入目标的本地访问材料。
    pub prepare_admission_target_access:
        Arc<dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort>,
    /// 准备 Sponsor 端的 Space 准入安全状态。
    pub prepare_sponsor_admission_security: Arc<dyn PrepareSponsorAdmissionSecurityPort>,
    /// 激活 Sponsor 端已准备的 Space 准入安全状态。
    pub activate_sponsor_admission_security: Arc<dyn ActivateSponsorAdmissionSecurityPort>,
    /// 激活完成辅助方的 Space 准入安全状态。
    pub activate_completion_helper_admission_security:
        Arc<dyn ActivateCompletionHelperAdmissionSecurityPort>,
    /// 从目标 GroupInfo 准备成员分支恢复的接收方状态。
    pub prepare_membership_branch_recovery_recipient:
        Arc<dyn PrepareMembershipBranchRecoveryRecipientPort>,
    /// 导出、准备并提交成员分支恢复材料。
    pub prepare_membership_branch_recovery_material:
        Arc<dyn PrepareMembershipBranchRecoveryMaterialPort>,
    /// 撤销群组成员并跟踪相关密钥世代更新。
    pub group_revocation: Arc<dyn GroupRevocationPort>,
    /// 启动旧 Space 的群组安全状态并跟踪重新准入确认。
    pub group_bootstrap: Arc<dyn uc_core::membership::GroupBootstrapPort>,
    /// 查询 Space 成员集合的当前保护状态。
    pub space_protection: Arc<dyn uc_core::membership::SpaceProtectionStatusPort>,
}

/// 安全领域端口组。
#[derive(Clone)]
pub struct SecurityPorts {
    /// 读写宿主安全存储中密钥材料的端口。
    pub secure_storage: Arc<dyn SecureStoragePort>,
    /// 探测 profile 密钥当前是否可访问的端口。
    pub profile_key_access_probe: Arc<dyn ProbeProfileKeyAccessPort>,
    /// Space 初始化、解锁、锁定、会话恢复与子密钥派生等窄意图端口。
    ///
    /// 消费者只应依赖实际调用的端口，不应持有全能 Space 访问接口。
    pub space_access_ports: SpaceAccessPorts,
    /// 使用适配器内部管理的端到端会话加解密 V3 剪贴板传输字节。
    pub transfer_cipher: Arc<dyn uc_core::ports::security::TransferCipherPort>,
    /// 为配对流程生成身份指纹的工厂端口。
    pub fingerprint: Arc<dyn uc_core::ports::security::IdentityFingerprintFactoryPort>,
}

/// 设备领域端口组，包含配对所需能力。
#[derive(Clone)]
pub struct DevicePorts {
    /// 读取本机稳定设备身份的端口。
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    /// 已准入 Space 成员的权威仓储端口。
    ///
    /// 成员身份与同步偏好仅由该仓储持久化，不再存在独立的已配对设备仓储。
    pub member_repo: Arc<dyn MemberRepositoryPort>,
}

/// 接收端文件传输投影的意图端口集合（ADR-009）。
///
/// 组合根将同一个 Diesel 适配器投影为这些窄端口，下游消费者只接收
/// 实际需要的能力。
#[derive(Clone)]
pub struct FileTransferPorts {
    /// 在接收 worker 读写状态前完成一次性明文残留清理。
    pub privacy_maintenance: Arc<dyn EnsureFileTransferPrivacyMaintenancePort>,
    /// 幂等记录接收端待处理文件传输。
    pub record: Arc<dyn RecordReceiverTransferPort>,
    /// 在条目归属尚未确定时创建临时接收记录。
    pub seed_provisional: Arc<dyn SeedProvisionalReceivePort>,
    /// 回填临时接收记录的受管缓存路径。
    pub update_provisional_path: Arc<dyn UpdateProvisionalReceivePathPort>,
    /// 列出需要恢复处理的临时接收记录。
    pub list_provisional: Arc<dyn ListProvisionalReceivesPort>,
    /// 将临时接收记录归入具体 attempt 或作为已完整持有内容丢弃。
    pub finalize_provisional: Arc<dyn FinalizeProvisionalReceivePort>,
    /// 查询条目下所有受跟踪文件传输的聚合状态。
    pub entry_summary: Arc<dyn GetEntryTransferSummaryPort>,
    /// 按传输 ID 查询其所属的剪贴板条目 ID。
    pub find_entry_id: Arc<dyn FindEntryIdForTransferPort>,
    /// 按传输 ID 查询其所属的接收 attempt ID。
    pub find_attempt_id: Arc<dyn FindAttemptIdForTransferPort>,
    /// 列出超过待处理或传输中截止时间的未完成传输。
    pub list_expired: Arc<dyn ListExpiredInflightTransfersPort>,
    /// 将单个或全部未完成传输终结为失败。
    pub fail_inflight: Arc<dyn FailInflightTransfersPort>,
    /// 取消指定目录接收 attempt 下的全部非终态传输。
    pub cancel_attempt: Arc<dyn CancelDirectoryAttemptTransfersPort>,
}

/// 目录接收 attempt、发布与临时产物结算的端口集合。
#[derive(Clone)]
pub struct DirectoryReceivePorts {
    /// 读取指定条目当前的接收 attempt。
    pub get_attempt: Arc<dyn GetEntryAttemptPort>,
    /// 列出所有尚未进入终态的接收 attempt。
    pub list_attempts: Arc<dyn ListNonTerminalAttemptsPort>,
    /// 原子记录目录发布阶段与完整的暂存根到最终根映射。
    pub record_publish: Arc<dyn RecordDirectoryPublishPort>,
    /// 读取指定目录接收 attempt 的发布恢复记录。
    pub get_publish: Arc<dyn GetDirectoryPublishRecordPort>,
    /// 开始首次接收，或在重投时替换精确匹配的已终结 attempt。
    pub begin_receive: Arc<dyn BeginReceiveAttemptPort>,
    /// 将精确匹配且正在接收的 attempt 领取为提交结算负责人。
    pub claim_commit: Arc<dyn ClaimReceiveCommitPort>,
    /// 请求取消接收 attempt，但不直接完成结算。
    pub request_cancel: Arc<dyn RequestReceiveCancellationPort>,
    /// 领取指定接收 attempt 的失败结算责任。
    pub begin_failure: Arc<dyn BeginReceiveFailurePort>,
    /// 持久化接收 attempt 产生的临时产物及其结算状态。
    pub record_artifacts: Arc<dyn RecordReceiveArtifactsPort>,
    /// 列出需要在恢复期继续处理的未结算接收产物。
    pub list_unsettled_artifacts: Arc<dyn ListUnsettledReceiveArtifactsPort>,
    /// 在一个事务中提交入站接收结算及其关联状态。
    pub commit_inbound: Arc<dyn CommitInboundReceivePort>,
    /// 查询条目当前远程入站 attempt 的聚合进度。
    pub entry_progress: Arc<dyn GetEntryReceiveProgressPort>,
}

/// 存储领域端口组，包含 Blob、缩略图与文件接收状态。
#[derive(Clone)]
pub struct StoragePorts {
    /// 按 Blob ID 读取内容的端口。
    pub blob_store: Arc<dyn BlobReaderPort>,
    /// 写入 Blob 内容并返回其身份的端口。
    pub blob_writer: Arc<dyn BlobWriterPort>,
    /// 从路径导入 Blob 并同时返回内容哈希的端口。
    ///
    /// 捕获流程使用内容哈希从设备无关的文件内容派生文件条目快照身份。
    pub blob_content_ingest: Arc<dyn BlobContentIngestPort>,
    /// 持久化由捕获流程构建的文件类条目逐项清单。
    pub entry_file_set_repo: Arc<dyn EntryFileSetRepositoryPort>,
    /// 持久化与读取条目缩略图的端口。
    pub thumbnail_repo: Arc<dyn ThumbnailRepositoryPort>,
    /// 从剪贴板内容生成缩略图的端口。
    pub thumbnail_generator: Arc<dyn ThumbnailGeneratorPort>,
    /// 接收端文件传输投影端口。
    pub file_transfer: FileTransferPorts,
    /// 目录接收、发布与产物结算端口。
    pub directory_receive: DirectoryReceivePorts,
}

/// 搜索领域端口组。
///
/// 集中携带必须共同装配的加密索引、存储维护、搜索密钥派生与文本处理
/// 能力，避免上层调用方自行拼装搜索基础设施。
#[derive(Clone)]
pub struct SearchPorts {
    /// 经写入协调包装的加密搜索索引，用于查询及增量写入、删除。
    pub search_index: Arc<dyn SearchIndexPort>,
    /// 执行明文残留清理等一次性搜索存储维护的端口。
    ///
    /// 它与 `search_index` 来自同一个具体适配器，但只暴露维护能力。
    pub search_maintenance: Arc<dyn SearchIndexMaintenancePort>,
    /// 以 profile 为作用域，通过 HKDF-SHA256 派生 HMAC 搜索密钥。
    pub search_key_derivation: Arc<dyn SearchKeyDerivationPort>,
    /// 为构建搜索文档执行文本提取与分词的处理流水线。
    pub search_pipeline: Arc<dyn SearchPipelinePort>,
    /// 未经增量写门禁包装的原始索引，仅供持有独占门禁的重建流程使用。
    rebuild_index: Arc<dyn SearchIndexPort>,
    /// 协调增量写入与全量重建的共享变更门禁。
    mutation_gate: Arc<SearchMutationGate>,
}

impl SearchPorts {
    /// 构造共享同一变更门禁的搜索端口组。
    ///
    /// 返回的 `search_index` 已包装增量写协调；原始索引仅保留给受控的重建流程。
    pub fn new(
        search_index: Arc<dyn SearchIndexPort>,
        search_maintenance: Arc<dyn SearchIndexMaintenancePort>,
        search_key_derivation: Arc<dyn SearchKeyDerivationPort>,
        search_pipeline: Arc<dyn SearchPipelinePort>,
    ) -> Self {
        let mutation_gate = Arc::new(SearchMutationGate::new());
        let coordinated_index = Arc::new(CoordinatedSearchIndex::new(
            Arc::clone(&search_index),
            Arc::clone(&mutation_gate),
        ));
        Self {
            search_index: coordinated_index,
            search_maintenance,
            search_key_derivation,
            search_pipeline,
            rebuild_index: search_index,
            mutation_gate,
        }
    }

    /// 返回全量重建专用的原始索引与共享变更门禁。
    pub(crate) fn rebuild_coordination(
        &self,
    ) -> (Arc<dyn SearchIndexPort>, Arc<SearchMutationGate>) {
        (
            Arc::clone(&self.rebuild_index),
            Arc::clone(&self.mutation_gate),
        )
    }
}

/// 系统领域端口组，包含时钟、内容哈希与缓存文件系统能力。
#[derive(Clone)]
pub struct SystemPorts {
    /// 提供可测试时间的时钟端口。
    pub clock: Arc<dyn ClockPort>,
    /// 计算稳定内容哈希的端口。
    pub hash: Arc<dyn ContentHashPort>,
    /// 读写受管缓存文件的文件系统端口。
    pub cache_fs: Arc<dyn uc_core::ports::cache_fs::CacheFsPort>,
}

/// Application 对象图的完整依赖分组。
///
/// 此结构体仅打包组合根已选定的必需能力，不是 Builder，也不执行隐式
/// 装配。除明确使用 `Option` 表达的能力外，字段均为必需依赖。
///
/// 所有字段都支持克隆，因此派生 `Clone` 以便在进程内复用同一份已装配
/// 对象图，而无需重新执行依赖装配。
#[derive(Clone)]
pub struct ApplicationDeps {
    /// Application 稳定目录输入；领域 assembly 不再向 Engine 重新请求路径。
    pub paths: crate::facade::AppPaths,
    /// Engine 选择的 relay 连通性诊断端口；Settings 决定是否及如何使用。
    pub relay_diagnostic: Option<Arc<dyn crate::settings::RelayDiagnosticPort>>,
    /// Application 内部各领域 assembly 共享的宿主事件出口。
    pub host_event_bus: Arc<crate::facade::HostEventBus>,
    /// 持久化并读取文件传输事件流的端口。
    pub file_transfer_event_store: Arc<dyn uc_core::file_transfer::FileTransferEventStorePort>,
    /// 安全清理接收 attempt 所有临时产物的宿主端口。
    pub receive_artifact_cleanup: Arc<dyn CleanupReceiveArtifactsPort>,
    /// 解析入站文件最终保存目录的宿主端口。
    pub receive_save_dir: Arc<dyn ResolveInboundSaveDirPort>,
    /// 启动剪贴板后台 worker 并将其注册到任务注册表的端口。
    pub clipboard_background: Arc<dyn crate::clipboard::assembly::ClipboardBackgroundPort>,
    /// 读写本地已信任 peer 记录的仓储端口。
    pub trusted_peer_repo: Arc<dyn uc_core::TrustedPeerRepositoryPort>,
    /// 记录剪贴板条目向各 peer 投递状态的仓储端口。
    pub entry_delivery_repo: Arc<dyn EntryDeliveryRepositoryPort>,
    /// 剪贴板领域端口。
    pub clipboard: ClipboardPorts,
    /// 安全领域端口。
    pub security: SecurityPorts,
    /// 设备与配对领域端口。
    pub device: DevicePorts,
    /// 持久化并恢复当前未完成 Space 重建目标的端口。
    pub space_rebuild_progress: Arc<dyn SpaceRebuildProgressPort>,
    /// 读写 profile 是否需要重新配对的持久状态端口。
    pub re_pairing_state_store: Arc<dyn RePairingStateStorePort>,
    /// 读取当前 Space 身份及旧 profile 隔离需求的端口。
    pub current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    /// 在首次初始化完成后激活初始 Space 的端口。
    pub initial_space_activation: Arc<dyn InitialSpaceActivationPort>,
    /// 整机配置迁移的被动能力输入；具体 facade 由 Settings assembly 构造。
    pub config_migration: ConfigMigrationDeps,
    /// 持久化上次运行的产品应用版本，供启动期升级检测读取和比较。
    pub app_version_state: Arc<dyn AppVersionStatePort>,
    /// 持久化 Engine 内部 Space 结构版本，与产品应用版本游标分离。
    pub engine_version_state: Arc<dyn uc_core::ports::EngineVersionStatePort>,
    /// 持久化首次剪贴板或文件同步事件是否已上报，并原子完成去重。
    ///
    /// 调用方仅在标记操作返回 `Ok(true)` 时上报首次同步事件。
    pub first_sync_state: Arc<dyn FirstSyncStatePort>,
    /// 存储领域端口。
    pub storage: StoragePorts,
    /// 读写应用设置的横切端口。
    pub settings: Arc<dyn SettingsPort>,
    /// 系统时钟、内容哈希与缓存文件系统端口。
    pub system: SystemPorts,
    /// 搜索索引、密钥派生与文本处理端口。
    pub search: SearchPorts,
    /// 接收产品分析事件的横切上报端口。
    ///
    /// 组合根使用带开关的 observer 包装具体 sink。调用方只需提交事件，
    /// 不需重复查询 `usage_analytics_enabled`。
    pub analytics: Arc<dyn AnalyticsPort>,
}
