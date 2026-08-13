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
    BootstrapId, DeviceVisibilityGatePort, GroupBootstrapPort, GroupBootstrapResult,
    LegacyBootstrapStatus, MemberProtectionStatus as CoreMemberProtectionStatus,
    MemberRepositoryPort, MembershipEventId, RemovalDecision,
    SpaceProtectionMode as CoreSpaceProtectionMode, SpaceProtectionSnapshot,
    SpaceProtectionStatusPort,
};
use uc_core::ports::{ConnectionChannelPort, LocalIdentityPort, PresenceEvent, PresencePort};
use uc_core::DeviceId;

use crate::facade::roster::commands::{
    apply_member_sync_preferences_patch, LegacyBootstrapState, LegacyBootstrapView,
    MemberProtectionStatusView, MemberProtectionView, MemberSummary, MemberSyncPreferencesPatch,
    MemberSyncPreferencesView, PeerSnapshotView, RosterEntry, SpaceProtectionModeView,
    SpaceProtectionView,
};
use crate::facade::roster::errors::RosterError;
use crate::space::convergence::WorkspaceConvergenceError;
use uc_core::membership::WorkspaceSnapshot;

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
    workspace_convergence: Option<Arc<crate::space::convergence::WorkspaceConvergence>>,
    visibility_gate: Option<Arc<dyn DeviceVisibilityGatePort>>,
}

impl MemberRosterFacade {
    pub fn new(deps: MemberRosterDeps) -> Self {
        Self {
            member_repo: deps.member_repo,
            local_identity: deps.local_identity,
            presence: deps.presence,
            connection_channel: deps.connection_channel,
            group_bootstrap: None,
            space_protection: None,
            workspace_convergence: None,
            visibility_gate: None,
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

    pub fn with_convergence(
        mut self,
        convergence: Arc<crate::space::convergence::assembly::SpaceConvergenceAssembly>,
    ) -> Self {
        self.visibility_gate = Some(convergence.device_visibility_gate());
        self.workspace_convergence = Some(Arc::clone(&convergence.workspace));
        self
    }

    #[cfg(test)]
    fn with_visibility_gate(mut self, visibility_gate: Arc<dyn DeviceVisibilityGatePort>) -> Self {
        self.visibility_gate = Some(visibility_gate);
        self
    }

    pub fn subscribe_workspace_convergence(&self) -> broadcast::Receiver<WorkspaceSnapshot> {
        self.workspace_convergence
            .as_ref()
            .map(|convergence| convergence.subscribe())
            .unwrap_or_else(|| {
                let (sender, _) = broadcast::channel(1);
                sender.subscribe()
            })
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

    /// 列出对外有效 peer 快照。该方法复用 roster + presence 聚合规则，并排除
    /// 已被本机移除的旧成员实例，避免原始成员记录重新暴露失效设备。
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
            if let Some(visibility_gate) = &self.visibility_gate {
                if visibility_gate
                    .is_hidden_from_device_lists(&entry.device_id)
                    .await
                {
                    continue;
                }
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

    /// 提交一次目标成员移除(ADR-016 调用方唯一提交入口)。
    ///
    /// 返回的完整工作空间快照只表示本机已生效、正在收敛;完成以保留成员
    /// 实际取得同一安全状态为准。
    pub async fn submit_member_removal(
        &self,
        target: &str,
    ) -> Result<WorkspaceSnapshot, RosterError> {
        let convergence = self
            .workspace_convergence
            .as_ref()
            .ok_or(RosterError::MembershipReconciliationUnavailable)?;
        let target = DeviceId::new(target);
        convergence
            .submit_removal(&target)
            .await
            .map_err(map_workspace_convergence_error)
    }

    /// 记录本机对已收到成员移除的唯一决定。
    pub async fn decide_membership_removal(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<WorkspaceSnapshot, RosterError> {
        let convergence = self
            .workspace_convergence
            .as_ref()
            .ok_or(RosterError::MembershipReconciliationUnavailable)?;
        convergence
            .decide_membership_removal(removal_event_id, decision)
            .await
            .map_err(map_workspace_convergence_error)
    }

    /// 查询当前完整工作空间收敛状态(一次查询恢复完整快照,不要求拼接事件)。
    pub async fn query_workspace_convergence(&self) -> Result<WorkspaceSnapshot, RosterError> {
        let convergence = self
            .workspace_convergence
            .as_ref()
            .ok_or(RosterError::MembershipReconciliationUnavailable)?;
        convergence
            .query()
            .await
            .map_err(map_workspace_convergence_error)
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

fn map_workspace_convergence_error(error: WorkspaceConvergenceError) -> RosterError {
    match error {
        WorkspaceConvergenceError::SelfTarget => RosterError::MemberRemovalInvalidInput,
        WorkspaceConvergenceError::UnknownTarget => RosterError::MemberRemovalTargetNotFound,
        error => RosterError::MemberRemoval(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use chrono::Utc;
    use uc_core::membership::{MemberSyncPreferences, MembershipError, SpaceMember};
    use uc_core::ports::{LocalIdentityError, ReachabilityState};
    use uc_core::security::IdentityFingerprint;

    struct Members(Vec<SpaceMember>);

    #[async_trait]
    impl MemberRepositoryPort for Members {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self
                .0
                .iter()
                .find(|member| member.device_id == *device_id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.0.clone())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(true)
        }
    }

    struct LocalIdentity(IdentityFingerprint);

    #[async_trait]
    impl LocalIdentityPort for LocalIdentity {
        async fn create(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
            Ok(self.0.clone())
        }

        async fn ensure(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
            Ok(self.0.clone())
        }

        async fn get_current_fingerprint(
            &self,
        ) -> Result<Option<IdentityFingerprint>, LocalIdentityError> {
            Ok(Some(self.0.clone()))
        }
    }

    struct StaticPresence;

    #[async_trait]
    impl PresencePort for StaticPresence {
        async fn ensure_reachable(
            &self,
            _device_id: &DeviceId,
        ) -> Result<ReachabilityState, uc_core::ports::PresenceError> {
            Ok(ReachabilityState::Online)
        }

        async fn current_state(&self, _device_id: &DeviceId) -> ReachabilityState {
            ReachabilityState::Online
        }

        fn subscribe(&self) -> broadcast::Receiver<PresenceEvent> {
            broadcast::channel(1).1
        }
    }

    struct RemovedTarget(DeviceId);

    #[async_trait]
    impl DeviceVisibilityGatePort for RemovedTarget {
        async fn is_hidden_from_device_lists(&self, device_id: &DeviceId) -> bool {
            *device_id == self.0
        }
    }

    fn fingerprint(value: &str) -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string(value).unwrap()
    }

    fn member(device_id: &str, name: &str, fingerprint: IdentityFingerprint) -> SpaceMember {
        SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: name.to_owned(),
            identity_fingerprint: fingerprint,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    #[tokio::test]
    async fn peer_snapshots_exclude_a_locally_removed_member() {
        let local = fingerprint("AAAAAAAAAAAAAAAA");
        let roster = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(Members(vec![
                member("alice", "A", local.clone()),
                member("bob", "B", fingerprint("BBBBBBBBBBBBBBBB")),
                member("charlie", "C", fingerprint("CCCCCCCCCCCCCCCC")),
            ])),
            local_identity: Arc::new(LocalIdentity(local)),
            presence: Arc::new(StaticPresence),
            connection_channel: None,
        })
        .with_visibility_gate(Arc::new(RemovedTarget(DeviceId::new("bob"))));

        let snapshots = roster.list_peer_snapshots().await.unwrap();
        assert_eq!(
            snapshots
                .into_iter()
                .map(|snapshot| snapshot.peer_id)
                .collect::<Vec<_>>(),
            vec!["charlie"]
        );
    }
}
