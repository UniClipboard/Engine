//! `MemberRosterFacade` —— 查询路径入口,不做拨号编排。
//!
//! ## 职责范围
//!
//! * `list_with_peer_reachability` —— 转发 Space roster 查询。纯读,不拨号。
//! * `subscribe_peer_reachability_events` —— `PeerReachabilityPort::subscribe` 的 thin 转发。
//!
//! ## 刻意不做
//!
//! * 主动拨号 —— T6 `EnsureReachableAllUseCase` 在 F1 hook 里统一触发;
//!   查询路径不背"触发副作用"的责任。
//! * rename / revoke —— Phase 3 membership 变更能力,Slice 2 不涉及。
//! * last_seen_at 汇总 —— `PeerReachabilityPort` 当前不追踪时间戳,加了也是永远
//!   `None`,省了先。

use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::instrument;

use uc_core::membership::{
    MemberProtectionStatus as CoreMemberProtectionStatus, MemberRepositoryPort,
    SpaceProtectionMode as CoreSpaceProtectionMode, SpaceProtectionSnapshot,
    SpaceProtectionStatusPort,
};
use uc_core::ports::{
    ConnectionChannelPort, LocalIdentityPort, PeerReachabilityChanged, PeerReachabilityPort,
};
use uc_core::DeviceId;

use crate::deps::CurrentSpaceMemberScopePort;
use crate::facade::roster::commands::{
    apply_member_sync_preferences_patch, MemberProtectionStatusView, MemberProtectionView,
    MemberSummary, MemberSyncPreferencesPatch, MemberSyncPreferencesView, PeerSnapshotView,
    SpaceProtectionModeView, SpaceProtectionView,
};
use crate::facade::roster::errors::RosterError;
use crate::space::{QueryMemberRosterError, QueryMemberRosterUseCase, RosterEntry};

/// 构造 `MemberRosterFacade` 时需要的 port 束。对齐 `SpaceFacadeDeps`
/// 的风格,便于 bootstrap 分步 construct 各 facade。
pub(crate) struct MemberRosterDeps {
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub local_identity: Arc<dyn LocalIdentityPort>,
    pub peer_reachability: Arc<dyn PeerReachabilityPort>,
    /// Phase 96 INDIC-01:连接通道单一真相源。`Option` 是为了 CLI / 测试
    /// 路径不强制构造 iroh adapter —— 缺省时 `list_peer_snapshots` 把
    /// channel 填成 `Unknown` 透传给 UI,UI 显式可见而非误判。
    pub connection_channel: Option<Arc<dyn ConnectionChannelPort>>,
    pub peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
}

/// Roster 查询门面 —— 见模块文档。
pub(crate) struct MemberRosterFacade {
    member_repo: Arc<dyn MemberRepositoryPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    peer_reachability: Arc<dyn PeerReachabilityPort>,
    connection_channel: Option<Arc<dyn ConnectionChannelPort>>,
    space_protection: Option<Arc<dyn SpaceProtectionStatusPort>>,
    peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    query_member_roster: QueryMemberRosterUseCase,
}

impl MemberRosterFacade {
    pub fn new(deps: MemberRosterDeps) -> Self {
        let query_member_roster = QueryMemberRosterUseCase::new(
            Arc::clone(&deps.member_repo),
            Arc::clone(&deps.local_identity),
            Arc::clone(&deps.peer_reachability),
            Arc::clone(&deps.peer_scope),
        );
        Self {
            member_repo: deps.member_repo,
            local_identity: deps.local_identity,
            peer_reachability: deps.peer_reachability,
            connection_channel: deps.connection_channel,
            space_protection: None,
            peer_scope: deps.peer_scope,
            query_member_roster,
        }
    }

    pub fn with_space_protection(
        mut self,
        space_protection: Arc<dyn SpaceProtectionStatusPort>,
    ) -> Self {
        self.space_protection = Some(space_protection);
        self
    }

    /// 转发当前 Space 的完整成员名单查询。
    pub async fn list_with_peer_reachability(&self) -> Result<Vec<RosterEntry>, RosterError> {
        self.query_member_roster
            .execute()
            .await
            .map_err(|error| match error {
                QueryMemberRosterError::MemberRepository { source } => {
                    RosterError::MemberRepository(source.to_string())
                }
                QueryMemberRosterError::MemberScope { .. } => {
                    RosterError::MembershipReconciliationUnavailable
                }
                QueryMemberRosterError::LocalIdentity { source } => {
                    RosterError::LocalIdentity(source.to_string())
                }
            })
    }

    /// 列出成员摘要。该方法面向 daemon/http 等外部入口,只返回应用层值对象。
    #[instrument(skip_all)]
    pub async fn list_members(&self) -> Result<Vec<MemberSummary>, RosterError> {
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?;
        let scope = self
            .peer_scope
            .snapshot()
            .await
            .map_err(|_| RosterError::MembershipReconciliationUnavailable)?;
        let local_fp = self
            .local_identity
            .get_current_fingerprint()
            .await
            .map_err(|err| RosterError::LocalIdentity(err.to_string()))?;

        // roster 展示已验证历史中的全部当前成员；paused 只限制通信资格，
        // 不能让离线拓扑中由历史引入的合法成员从名单中消失。
        Ok(members
            .into_iter()
            .filter(|member| {
                local_fp
                    .as_ref()
                    .is_some_and(|fingerprint| fingerprint == &member.identity_fingerprint)
                    || scope.usable_peer_device_ids.contains(&member.device_id)
                    || scope
                        .paused_peer_devices
                        .iter()
                        .any(|peer| peer.device_id == member.device_id)
            })
            .map(|member| MemberSummary {
                device_id: member.device_id.as_str().to_string(),
                device_name: member.device_name,
            })
            .collect())
    }

    /// 列出对外有效 peer 快照。该方法复用 roster + peer_reachability 聚合规则，并排除
    /// 已被本机移除的旧成员实例，避免原始成员记录重新暴露失效设备。
    ///
    /// Phase 96:每条 entry 顺带带上 `channel`(Direct/Relay/Offline/
    /// Unknown)。`connection_channel` port 缺省时降级为 `Unknown` —— UI
    /// 显式可见,优于猜测(Pitfall 4)。
    #[instrument(skip_all)]
    pub async fn list_peer_snapshots(&self) -> Result<Vec<PeerSnapshotView>, RosterError> {
        let entries = self.list_with_peer_reachability().await?;
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
        SpaceProtectionView { mode, members }
    }

    /// `PeerReachabilityPort::subscribe` 的 thin 转发。
    ///
    /// 每次调用拿一个新 receiver,共享 adapter 的 broadcast 源。标准
    /// `tokio::sync::broadcast` lag 语义:某个 subscriber 落后 capacity 时
    /// 最老的事件会被丢——acceptable,因为最新状态总能通过
    /// `list_with_peer_reachability` 或再来一次订阅重建。
    pub fn subscribe_peer_reachability_events(
        &self,
    ) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.peer_reachability.subscribe()
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

    struct StaticPeerReachability;

    #[async_trait]
    impl PeerReachabilityPort for StaticPeerReachability {
        async fn ensure_reachable(
            &self,
            _device_id: &DeviceId,
        ) -> Result<ReachabilityState, uc_core::ports::PeerReachabilityError> {
            Ok(ReachabilityState::Online)
        }

        async fn current_state(&self, _device_id: &DeviceId) -> ReachabilityState {
            ReachabilityState::Online
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
            broadcast::channel(1).1
        }
    }

    struct FixedPeerScope(Vec<DeviceId>);

    #[async_trait]
    impl CurrentSpaceMemberScopePort for FixedPeerScope {
        async fn snapshot(
            &self,
        ) -> Result<crate::deps::CurrentSpaceMemberScope, crate::deps::CurrentSpaceMemberScopeError>
        {
            Ok(crate::deps::CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: self.0.clone(),
                paused_peer_devices: Vec::new(),
            })
        }
    }

    struct PausedPeerScope(DeviceId);

    #[async_trait]
    impl CurrentSpaceMemberScopePort for PausedPeerScope {
        async fn snapshot(
            &self,
        ) -> Result<crate::deps::CurrentSpaceMemberScope, crate::deps::CurrentSpaceMemberScopeError>
        {
            Ok(crate::deps::CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: Vec::new(),
                paused_peer_devices: vec![crate::deps::PausedSpaceMember {
                    device_id: self.0.clone(),
                    reason: crate::deps::SpaceMemberPauseReason::RelationshipUnconfirmed,
                }],
            })
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
            peer_reachability: Arc::new(StaticPeerReachability),
            connection_channel: None,
            peer_scope: Arc::new(FixedPeerScope(vec![DeviceId::new("charlie")])),
        });

        let snapshots = roster.list_peer_snapshots().await.unwrap();
        assert_eq!(
            snapshots
                .into_iter()
                .map(|snapshot| snapshot.peer_id)
                .collect::<Vec<_>>(),
            vec!["charlie"]
        );

        let members = roster.list_members().await.unwrap();
        assert_eq!(
            members
                .into_iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>(),
            vec!["alice", "charlie"]
        );
    }

    #[tokio::test]
    async fn member_roster_includes_a_history_member_while_reconciliation_is_paused() {
        let local = fingerprint("AAAAAAAAAAAAAAAA");
        let roster = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(Members(vec![
                member("alice", "A", local.clone()),
                member("charlie", "C", fingerprint("CCCCCCCCCCCCCCCC")),
            ])),
            local_identity: Arc::new(LocalIdentity(local)),
            peer_reachability: Arc::new(StaticPeerReachability),
            connection_channel: None,
            peer_scope: Arc::new(PausedPeerScope(DeviceId::new("charlie"))),
        });

        let members = roster.list_members().await.unwrap();

        assert_eq!(
            members
                .into_iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>(),
            vec!["alice", "charlie"]
        );
        assert_eq!(
            roster
                .list_peer_snapshots()
                .await
                .unwrap()
                .into_iter()
                .map(|snapshot| snapshot.peer_id)
                .collect::<Vec<_>>(),
            vec!["charlie"]
        );
    }
}
