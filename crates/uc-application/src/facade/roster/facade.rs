//! `MemberRosterFacade` —— 查询路径入口,不做拨号编排。
//!
//! ## 职责范围
//!
//! * `list_with_presence` —— `member_repo.list()` + `presence.current_state()` +
//!   `local_identity.get_current_fingerprint()` 聚合。纯读,不拨号。
//! * `subscribe_presence_events` —— `PresencePort::subscribe` 的 thin 转发。
//!
//! ## 刻意不做
//!
//! * 主动拨号 —— T6 `EnsureReachableAllUseCase` 在 F1 hook 里统一触发;
//!   查询路径不背"触发副作用"的责任。
//! * rename / revoke —— Phase 3 membership 变更能力,Slice 2 不涉及。
//! * last_seen_at 汇总 —— `PresencePort` 当前不追踪时间戳,加了也是永远
//!   `None`,省了先。

use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::instrument;

use uc_core::membership::{
    BootstrapId, GroupBootstrapPort, GroupBootstrapResult, LegacyBootstrapStatus,
    MemberProtectionStatus as CoreMemberProtectionStatus, MemberRepositoryPort,
    SpaceProtectionMode as CoreSpaceProtectionMode, SpaceProtectionSnapshot,
    SpaceProtectionStatusPort,
};
use uc_core::ports::{ConnectionChannelPort, LocalIdentityPort, PresenceEvent, PresencePort};
use uc_core::DeviceId;

use crate::facade::roster::commands::{
    apply_member_sync_preferences_patch, LegacyBootstrapState, LegacyBootstrapView,
    MemberProtectionStatusView, MemberProtectionView, MemberRemovalView, MemberSummary,
    MemberSyncPreferencesPatch, MemberSyncPreferencesView, PeerSnapshotView, RosterEntry,
    SpaceProtectionModeView, SpaceProtectionView,
};
use crate::facade::roster::errors::RosterError;
use crate::member_removal::RemovalCoordinatorError;

/// 构造 `MemberRosterFacade` 时需要的 port 束。对齐 `SpaceFacadeDeps`
/// 的风格,便于 bootstrap 分步 construct 各 facade。
pub struct MemberRosterDeps {
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub local_identity: Arc<dyn LocalIdentityPort>,
    pub presence: Arc<dyn PresencePort>,
    /// Phase 96 INDIC-01:连接通道单一真相源。`Option` 是为了 CLI / 测试
    /// 路径不强制构造 iroh adapter —— 缺省时 `list_peer_snapshots` 把
    /// channel 填成 `Unknown` 透传给 UI,UI 显式可见而非误判。
    pub connection_channel: Option<Arc<dyn ConnectionChannelPort>>,
}

/// Roster 查询门面 —— 见模块文档。
pub struct MemberRosterFacade {
    member_repo: Arc<dyn MemberRepositoryPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    presence: Arc<dyn PresencePort>,
    connection_channel: Option<Arc<dyn ConnectionChannelPort>>,
    group_bootstrap: Option<Arc<dyn GroupBootstrapPort>>,
    space_protection: Option<Arc<dyn SpaceProtectionStatusPort>>,
    member_removal_events: broadcast::Sender<MemberRemovalView>,
    member_removal: Option<Arc<crate::member_removal::RemovalCoordinator>>,
}

impl MemberRosterFacade {
    pub fn new(deps: MemberRosterDeps) -> Self {
        let (member_removal_events, _) = broadcast::channel(64);
        Self {
            member_repo: deps.member_repo,
            local_identity: deps.local_identity,
            presence: deps.presence,
            connection_channel: deps.connection_channel,
            group_bootstrap: None,
            space_protection: None,
            member_removal_events,
            member_removal: None,
        }
    }

    pub fn with_group_bootstrap(mut self, group_bootstrap: Arc<dyn GroupBootstrapPort>) -> Self {
        self.group_bootstrap = Some(group_bootstrap);
        self
    }

    pub fn with_space_protection(
        mut self,
        space_protection: Arc<dyn SpaceProtectionStatusPort>,
    ) -> Self {
        self.space_protection = Some(space_protection);
        self
    }

    pub fn with_member_removal(
        mut self,
        member_removal: Arc<crate::member_removal::RemovalCoordinator>,
    ) -> Self {
        self.member_removal = Some(member_removal);
        self
    }

    pub fn subscribe_member_removal_events(&self) -> broadcast::Receiver<MemberRemovalView> {
        self.member_removal_events.subscribe()
    }

    pub fn start_member_removal_runtime(
        &self,
    ) -> Result<crate::member_removal::MemberRemovalRuntime, RosterError> {
        let coordinator = self
            .member_removal
            .as_ref()
            .ok_or(RosterError::MemberRemovalUnavailable)?;
        Ok(crate::member_removal::MemberRemovalRuntime::start(
            Arc::clone(coordinator),
            self.presence.subscribe(),
            self.member_removal_events.clone(),
        ))
    }

    /// 聚合当前所有成员 + 各自 presence 状态 + 本机标记。
    ///
    /// 读路径保证:`PresencePort::current_state` 按 port 契约是纯缓存读,
    /// 不会拨号 / 不会阻塞 IO。member_repo / local_identity 都是本地存
    /// 储读,整体延迟受 IO 限制但不受网络影响——可以被 UI 高频调用。
    ///
    /// `local_identity.get_current_fingerprint()` 返回 `Ok(None)` 表示本
    /// 机尚未创建身份(pre-A1 / pre-B2),此时所有 entry 都会标 `is_local
    /// == false`——对该窗口期通常没有成员记录所以影响微乎其微,属于
    /// 防御性路径。
    #[instrument(skip_all)]
    pub async fn list_with_presence(&self) -> Result<Vec<RosterEntry>, RosterError> {
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?;

        let local_fp = self
            .local_identity
            .get_current_fingerprint()
            .await
            .map_err(|err| RosterError::LocalIdentity(err.to_string()))?;

        let mut entries = Vec::with_capacity(members.len());
        for member in members {
            let is_local = local_fp
                .as_ref()
                .is_some_and(|fp| fp == &member.identity_fingerprint);
            let state = self.presence.current_state(&member.device_id).await;
            entries.push(RosterEntry {
                device_id: member.device_id,
                device_name: member.device_name,
                is_local,
                state,
            });
        }
        Ok(entries)
    }

    /// 列出成员摘要。该方法面向 daemon/http 等外部入口,只返回应用层值对象。
    #[instrument(skip_all)]
    pub async fn list_members(&self) -> Result<Vec<MemberSummary>, RosterError> {
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?;

        Ok(members
            .into_iter()
            .map(|member| MemberSummary {
                device_id: member.device_id.as_str().to_string(),
                device_name: member.device_name,
            })
            .collect())
    }

    /// 列出对外 peer 快照。该方法复用 roster + presence 聚合规则,并隐藏
    /// core `ReachabilityState` / `DeviceId` 等内部模型。
    ///
    /// Phase 96:每条 entry 顺带带上 `channel`(Direct/Relay/Offline/
    /// Unknown)。`connection_channel` port 缺省时降级为 `Unknown` —— UI
    /// 显式可见,优于猜测(Pitfall 4)。
    #[instrument(skip_all)]
    pub async fn list_peer_snapshots(&self) -> Result<Vec<PeerSnapshotView>, RosterError> {
        let entries = self.list_with_presence().await?;
        let mut snapshots = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.is_local {
                continue;
            }
            let path = match &self.connection_channel {
                Some(port) => port.path_for(&entry.device_id).await,
                None => uc_core::ports::ConnectionPath::default(),
            };
            snapshots.push(PeerSnapshotView {
                peer_id: entry.device_id.as_str().to_string(),
                device_name: if entry.device_name.is_empty() {
                    None
                } else {
                    Some(entry.device_name)
                },
                addresses: Vec::new(),
                is_paired: true,
                connected: matches!(entry.state, uc_core::ports::ReachabilityState::Online),
                pairing_state: "Trusted".to_string(),
                channel: path.channel,
                connection_address: path.address,
            });
        }
        Ok(snapshots)
    }

    /// 读取某个成员的同步偏好。调用方传入字符串设备 ID,不接触 core 类型。
    #[instrument(skip_all)]
    pub async fn get_sync_preferences(
        &self,
        device_id: &str,
    ) -> Result<MemberSyncPreferencesView, RosterError> {
        let device_id = DeviceId::new(device_id);
        let member = self
            .member_repo
            .get(&device_id)
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?
            .ok_or_else(|| RosterError::NotFound(device_id.as_str().to_string()))?;

        Ok(member.sync_preferences.into())
    }

    /// 局部更新某个成员的同步偏好。合并规则收敛在 application 层。
    #[instrument(skip_all)]
    pub async fn update_sync_preferences(
        &self,
        device_id: &str,
        patch: MemberSyncPreferencesPatch,
    ) -> Result<MemberSyncPreferencesView, RosterError> {
        let device_id = DeviceId::new(device_id);
        let existing = self
            .member_repo
            .get(&device_id)
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?
            .ok_or_else(|| RosterError::NotFound(device_id.as_str().to_string()))?;

        let updated_preferences =
            apply_member_sync_preferences_patch(existing.sync_preferences, patch);
        let updated = uc_core::SpaceMember {
            sync_preferences: updated_preferences,
            ..existing
        };

        self.member_repo
            .save(&updated)
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?;

        Ok(updated.sync_preferences.into())
    }

    /// 提交一次目标成员移除(ADR-015 调用方唯一提交入口)。
    ///
    /// 返回的摘要只表示本机已生效、正在收敛;完成以保留成员实际取得同一
    /// 安全状态为准。
    pub async fn submit_member_removal(
        &self,
        target: &str,
    ) -> Result<MemberRemovalView, RosterError> {
        let coordinator = self
            .member_removal
            .as_ref()
            .ok_or(RosterError::MemberRemovalUnavailable)?;
        let target = DeviceId::new(target);
        let summary = coordinator
            .submit_removal(&target, chrono::Utc::now().timestamp_millis())
            .await
            .map_err(map_member_removal_error)?;
        let view = MemberRemovalView::from_summary(summary);
        let _ = self.member_removal_events.send(view.clone());
        Ok(view)
    }

    /// 查询当前完整移除状态(一次查询恢复完整视图,不要求拼接事件)。
    pub async fn query_member_removal(&self) -> Result<MemberRemovalView, RosterError> {
        let coordinator = self
            .member_removal
            .as_ref()
            .ok_or(RosterError::MemberRemovalUnavailable)?;
        let summary = coordinator
            .query(chrono::Utc::now().timestamp_millis())
            .await
            .map_err(|error| RosterError::MemberRemoval(error.to_string()))?;
        Ok(MemberRemovalView::from_summary(summary))
    }

    pub async fn query_legacy_bootstrap(
        &self,
        bootstrap_id: &str,
    ) -> Result<Option<LegacyBootstrapView>, RosterError> {
        let group_bootstrap = self
            .group_bootstrap
            .as_ref()
            .ok_or(RosterError::Unavailable)?;
        let bootstrap_id = BootstrapId::from_string(bootstrap_id)
            .map_err(|error| RosterError::GroupBootstrap(error.to_string()))?;
        group_bootstrap
            .query_legacy_bootstrap(&bootstrap_id)
            .await
            .map(|result| result.map(Self::legacy_bootstrap_view))
            .map_err(|error| RosterError::GroupBootstrap(error.to_string()))
    }

    pub async fn resume_legacy_bootstraps(&self) -> Result<Vec<LegacyBootstrapView>, RosterError> {
        let Some(group_bootstrap) = &self.group_bootstrap else {
            return Ok(Vec::new());
        };
        group_bootstrap
            .resume_legacy_bootstraps(chrono::Utc::now().timestamp_millis())
            .await
            .map(|results| {
                results
                    .into_iter()
                    .map(Self::legacy_bootstrap_view)
                    .collect()
            })
            .map_err(|error| RosterError::GroupBootstrap(error.to_string()))
    }

    pub async fn query_space_protection(&self) -> Result<SpaceProtectionView, RosterError> {
        let space_protection = self
            .space_protection
            .as_ref()
            .ok_or(RosterError::Unavailable)?;
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|error| RosterError::MemberRepository(error.to_string()))?;
        let member_ids = members
            .into_iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        space_protection
            .query_space_protection(&member_ids)
            .await
            .map(Self::space_protection_view)
            .map_err(|error| RosterError::SpaceProtection(error.to_string()))
    }

    fn space_protection_view(snapshot: SpaceProtectionSnapshot) -> SpaceProtectionView {
        let mode = match snapshot.mode {
            CoreSpaceProtectionMode::Legacy => SpaceProtectionModeView::Legacy,
            CoreSpaceProtectionMode::Migrating => SpaceProtectionModeView::Migrating,
            CoreSpaceProtectionMode::Ready => SpaceProtectionModeView::Ready,
        };
        let members = snapshot
            .members
            .into_iter()
            .map(|member| MemberProtectionView {
                device_id: member.device_id.as_str().to_owned(),
                status: match member.status {
                    CoreMemberProtectionStatus::LegacyUnprotected => {
                        MemberProtectionStatusView::LegacyUnprotected
                    }
                    CoreMemberProtectionStatus::Protected => MemberProtectionStatusView::Protected,
                    CoreMemberProtectionStatus::AwaitingReadmission => {
                        MemberProtectionStatusView::AwaitingReadmission
                    }
                    CoreMemberProtectionStatus::RequiresReadmission => {
                        MemberProtectionStatusView::RequiresReadmission
                    }
                    CoreMemberProtectionStatus::RecoveryRequired => {
                        MemberProtectionStatusView::RecoveryRequired
                    }
                },
            })
            .collect();
        let legacy_bootstrap = snapshot
            .legacy_bootstrap
            .map(|progress| LegacyBootstrapView {
                bootstrap_id: progress.bootstrap_id.as_str().to_owned(),
                state: match progress.status {
                    LegacyBootstrapStatus::RecoveryRequired => {
                        LegacyBootstrapState::RecoveryRequired
                    }
                    LegacyBootstrapStatus::Complete => LegacyBootstrapState::Complete,
                    LegacyBootstrapStatus::Prepared
                    | LegacyBootstrapStatus::Staged
                    | LegacyBootstrapStatus::AwaitingReadmission => {
                        LegacyBootstrapState::AwaitingReadmission
                    }
                },
                pending_readmission: progress.pending_readmission,
            });
        SpaceProtectionView {
            mode,
            members,
            legacy_bootstrap,
        }
    }

    fn legacy_bootstrap_view(result: GroupBootstrapResult) -> LegacyBootstrapView {
        match result {
            GroupBootstrapResult::AwaitingReadmission {
                bootstrap_id,
                pending_members,
            } => LegacyBootstrapView {
                bootstrap_id: bootstrap_id.as_str().to_owned(),
                state: LegacyBootstrapState::AwaitingReadmission,
                pending_readmission: pending_members,
            },
            GroupBootstrapResult::Complete { bootstrap_id } => LegacyBootstrapView {
                bootstrap_id: bootstrap_id.as_str().to_owned(),
                state: LegacyBootstrapState::Complete,
                pending_readmission: 0,
            },
            GroupBootstrapResult::RecoveryRequired { bootstrap_id } => LegacyBootstrapView {
                bootstrap_id: bootstrap_id.as_str().to_owned(),
                state: LegacyBootstrapState::RecoveryRequired,
                pending_readmission: 0,
            },
        }
    }

    /// `PresencePort::subscribe` 的 thin 转发。
    ///
    /// 每次调用拿一个新 receiver,共享 adapter 的 broadcast 源。标准
    /// `tokio::sync::broadcast` lag 语义:某个 subscriber 落后 capacity 时
    /// 最老的事件会被丢——acceptable,因为最新状态总能通过
    /// `list_with_presence` 或再来一次订阅重建。
    pub fn subscribe_presence_events(&self) -> broadcast::Receiver<PresenceEvent> {
        self.presence.subscribe()
    }
}

fn map_member_removal_error(error: RemovalCoordinatorError) -> RosterError {
    match error {
        RemovalCoordinatorError::SelfTarget => RosterError::MemberRemovalInvalidInput,
        RemovalCoordinatorError::UnknownTarget => RosterError::MemberRemovalTargetNotFound,
        error => RosterError::MemberRemoval(error.to_string()),
    }
}
