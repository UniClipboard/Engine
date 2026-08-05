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
use tracing::{error, instrument, warn};

use uc_core::membership::{
    BootstrapId, GroupBootstrapPort, GroupBootstrapResult, GroupRevocationPort,
    GroupRevocationResult, GroupUpdateDispatchPort, KeyEpochError, LegacyBootstrapStatus,
    MemberProtectionStatus as CoreMemberProtectionStatus, MemberRepositoryPort, RevocationStatus,
    SpaceMember, SpaceProtectionMode as CoreSpaceProtectionMode, SpaceProtectionSnapshot,
    SpaceProtectionStatusPort,
};
use uc_core::ports::peer_address::PeerAddressRepositoryPort;
use uc_core::ports::{ConnectionChannelPort, LocalIdentityPort, PresenceEvent, PresencePort};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;
use uc_core::DeviceId;

use crate::facade::roster::commands::{
    apply_member_sync_preferences_patch, LegacyBootstrapState, LegacyBootstrapView,
    MemberProtectionStatusView, MemberProtectionView, MemberRevocationState, MemberRevocationView,
    MemberSummary, MemberSyncPreferencesPatch, MemberSyncPreferencesView, PeerSnapshotView,
    RosterEntry, SpaceProtectionModeView, SpaceProtectionView,
};
use crate::facade::roster::errors::RosterError;
use crate::group_update_delivery::{GroupUpdateDelivery, GroupUpdateDeliveryPort};

/// 构造 `MemberRosterFacade` 时需要的 port 束。对齐 `SpaceFacadeDeps`
/// 的风格,便于 bootstrap 分步 construct 各 facade。
pub struct MemberRosterDeps {
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    pub trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
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
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    presence: Arc<dyn PresencePort>,
    connection_channel: Option<Arc<dyn ConnectionChannelPort>>,
    group_revocation: Option<Arc<dyn GroupRevocationPort>>,
    group_bootstrap: Option<Arc<dyn GroupBootstrapPort>>,
    space_protection: Option<Arc<dyn SpaceProtectionStatusPort>>,
    group_update_dispatch: Option<Arc<dyn GroupUpdateDispatchPort>>,
    group_update_delivery: Option<Arc<dyn GroupUpdateDeliveryPort>>,
    member_revocation_events: broadcast::Sender<MemberRevocationView>,
}

impl MemberRosterFacade {
    pub fn new(deps: MemberRosterDeps) -> Self {
        let (member_revocation_events, _) = broadcast::channel(64);
        Self {
            member_repo: deps.member_repo,
            peer_addr_repo: deps.peer_addr_repo,
            trusted_peer_repo: deps.trusted_peer_repo,
            local_identity: deps.local_identity,
            presence: deps.presence,
            connection_channel: deps.connection_channel,
            group_revocation: None,
            group_bootstrap: None,
            space_protection: None,
            group_update_dispatch: None,
            group_update_delivery: None,
            member_revocation_events,
        }
    }

    pub fn new_with_group_revocation(
        deps: MemberRosterDeps,
        group_revocation: Arc<dyn GroupRevocationPort>,
    ) -> Self {
        let mut facade = Self::new(deps);
        facade.group_revocation = Some(group_revocation);
        facade
    }

    pub fn new_with_group_delivery(
        deps: MemberRosterDeps,
        group_revocation: Arc<dyn GroupRevocationPort>,
        group_update_dispatch: Arc<dyn GroupUpdateDispatchPort>,
    ) -> Self {
        let group_update_delivery: Arc<dyn GroupUpdateDeliveryPort> =
            Arc::new(GroupUpdateDelivery::new(
                Arc::clone(&group_revocation),
                Arc::clone(&group_update_dispatch),
            ));
        let mut facade = Self::new_with_group_revocation(deps, group_revocation);
        facade.group_update_delivery = Some(group_update_delivery);
        facade.group_update_dispatch = Some(group_update_dispatch);
        facade
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

    pub fn subscribe_member_revocation_events(&self) -> broadcast::Receiver<MemberRevocationView> {
        self.member_revocation_events.subscribe()
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

    /// 撤销成员。撤销语义由 application 层表达,daemon 不直接调用 use case。
    ///
    /// Facade 级联清理三张表,以维持设计意图的不变量
    /// `trusted_peer ⊆ member` 与 `peer_addr ⊆ member`:
    ///
    /// 1. `member_repo` —— 主体记录,先删;返回 `Ok(false)` 表示成员不存在
    ///    (`NotFound`),后续两步跳过。
    /// 2. `peer_addr_repo` —— 不删会让 `dispatch_entry` /
    ///    `ensure_reachable_all` 仍把已撤销设备当目标(见
    ///    `dispatch_entry.rs` module doc 关于 "avoids iterating ghost entries
    ///    in `member_repo` that never completed pairing" 的不变量)。
    /// 3. `trusted_peer_repo` —— 维持 `trusted_peer ⊆ member` 不变量:
    ///    残留的 trust 行会让本机继续把已撤销设备当可信对端。(#1023 之后
    ///    `TrustPeerUseCase::execute` 改为重配时显式替换,残留行不再挡死
    ///    重新配对,但级联清理仍是撤销语义的一部分。)`distrust_peer.rs`
    ///    注释里讲 "UI 的'解除配对'由 Facade 级联调用",这里就是那个级联。
    ///
    /// 失败处理:`member_repo.remove` 已成功后没法回滚,后续两步任一失败
    /// 都把错误抛给调用方,启动期 `reconcile_*` 会在下次 boot 兜底。两个
    /// remove port 都是 idempotent,缺行不会算 error。
    #[instrument(skip_all)]
    pub async fn revoke_member(
        &self,
        device_id: &str,
    ) -> Result<MemberRevocationView, RosterError> {
        let device_id = DeviceId::new(device_id);
        if let Some(current) = self.current_group_revocation().await? {
            if current.removed_device_ids().contains(&device_id) {
                return Ok(Self::revocation_view(current));
            }
            return Err(
                if current.status() == Some(RevocationStatus::RecoveryRequired) {
                    RosterError::MemberRemovalRecoveryRequired
                } else {
                    RosterError::MemberRemovalInProgress
                },
            );
        }
        let local_fingerprint = self.validate_remote_member(&device_id).await?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut revocation = if let Some(group_revocation) = &self.group_revocation {
            let members = self
                .member_repo
                .list()
                .await
                .map_err(|err| RosterError::MemberRepository(err.to_string()))?;
            if let Some(space_protection) = &self.space_protection {
                let member_ids = members
                    .iter()
                    .map(|member| member.device_id)
                    .collect::<Vec<_>>();
                let protection = space_protection
                    .query_space_protection(&member_ids)
                    .await
                    .map_err(|error| RosterError::SpaceProtection(error.to_string()))?;
                let target_status = protection
                    .members
                    .iter()
                    .find(|member| member.device_id == device_id)
                    .map(|member| member.status);
                if matches!(
                    target_status,
                    Some(
                        CoreMemberProtectionStatus::AwaitingReadmission
                            | CoreMemberProtectionStatus::RequiresReadmission
                    )
                ) {
                    if target_status == Some(CoreMemberProtectionStatus::AwaitingReadmission) {
                        let bootstrap = protection
                            .legacy_bootstrap
                            .ok_or(RosterError::Unavailable)?;
                        self.group_bootstrap
                            .as_ref()
                            .ok_or(RosterError::Unavailable)?
                            .withdraw_legacy_readmission(
                                &bootstrap.bootstrap_id,
                                &device_id,
                                now_ms,
                            )
                            .await
                            .map_err(|error| RosterError::GroupBootstrap(error.to_string()))?;
                    }
                    self.remove_member_records(&device_id).await?;
                    let result = MemberRevocationView {
                        revocation_id: None,
                        state: MemberRevocationState::Complete,
                        pending_recipients: 0,
                        removed_device_ids: vec![device_id.as_str().to_owned()],
                        pending_recipient_device_ids: Vec::new(),
                        updated_at_ms: now_ms,
                    };
                    self.publish_member_revocation(result.clone());
                    return Ok(result);
                }
            }
            let retained_recipients = members
                .into_iter()
                .filter(|candidate| candidate.device_id != device_id)
                .filter(|candidate| candidate.identity_fingerprint != local_fingerprint)
                .map(|candidate| candidate.device_id)
                .collect::<Vec<_>>();
            group_revocation
                .revoke_group_member(&device_id, &retained_recipients, now_ms)
                .await
                .map_err(|error| RosterError::GroupRevocation(error.to_string()))?
        } else {
            GroupRevocationResult::LocalOnly
        };
        if matches!(revocation, GroupRevocationResult::LocalOnly) {
            return Err(RosterError::LegacyBootstrapRequired);
        }
        self.publish_member_revocation(Self::revocation_view(revocation.clone()));
        self.remove_member_records(&device_id).await?;

        if self.retry_pending_space_group_updates().await.is_err() {
            warn!("pending space group updates remain deferred after member cleanup");
        }
        match self.retry_group_delivery(revocation.clone()).await {
            Ok(updated) => revocation = updated,
            Err(_) => {
                warn!("member revocation delivery remains deferred after cleanup");
            }
        }

        Ok(Self::revocation_view(revocation))
    }

    pub async fn current_member_revocation(
        &self,
    ) -> Result<Option<MemberRevocationView>, RosterError> {
        self.current_group_revocation()
            .await
            .map(|current| current.map(Self::revocation_view))
    }

    pub async fn continue_member_revocation(
        &self,
        revocation_id: &str,
        permanently_lost_device_ids: &[String],
    ) -> Result<MemberRevocationView, RosterError> {
        let revocation_id = uc_core::membership::RevocationId::from_string(revocation_id)
            .map_err(|_| RosterError::InvalidPermanentLossSelection)?;
        let current = self
            .current_group_revocation()
            .await?
            .ok_or_else(|| RosterError::NotFound(revocation_id.as_str().to_owned()))?;
        if current.revocation_id() != Some(&revocation_id) || permanently_lost_device_ids.is_empty()
        {
            return Err(RosterError::InvalidPermanentLossSelection);
        }
        let requested = permanently_lost_device_ids
            .iter()
            .map(DeviceId::new)
            .collect::<Vec<_>>();
        let unique = requested.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != requested.len() {
            return Err(RosterError::InvalidPermanentLossSelection);
        }
        let (already_removed, outstanding): (Vec<_>, Vec<_>) = requested
            .into_iter()
            .partition(|device_id| current.removed_device_ids().contains(device_id));
        for device_id in &already_removed {
            self.remove_member_records_idempotent(device_id).await?;
        }
        if outstanding.is_empty() {
            return Ok(Self::revocation_view(current));
        }
        if outstanding
            .iter()
            .any(|device_id| !current.pending_recipient_device_ids().contains(device_id))
        {
            return Err(RosterError::InvalidPermanentLossSelection);
        }
        let group_revocation = self
            .group_revocation
            .as_ref()
            .ok_or(RosterError::Unavailable)?;
        let mut result = group_revocation
            .continue_group_revocation(
                &revocation_id,
                &outstanding,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| match error {
                uc_core::membership::KeyEpochError::PermanentLossRecipientNotPending => {
                    RosterError::InvalidPermanentLossSelection
                }
                other => RosterError::GroupRevocation(other.to_string()),
            })?;
        self.publish_member_revocation(Self::revocation_view(result.clone()));
        for device_id in &outstanding {
            self.remove_member_records_idempotent(device_id).await?;
        }
        match self.retry_group_delivery(result.clone()).await {
            Ok(updated) => result = updated,
            Err(_) => {
                warn!("member revocation recovery delivery remains deferred");
            }
        }
        Ok(Self::revocation_view(result))
    }

    async fn current_group_revocation(&self) -> Result<Option<GroupRevocationResult>, RosterError> {
        let Some(group_revocation) = &self.group_revocation else {
            return Ok(None);
        };
        group_revocation
            .current_group_revocation()
            .await
            .map_err(|failure| {
                error!(
                    operation = "query_current_member_revocation",
                    failure_kind = Self::group_revocation_failure_kind(&failure),
                    "current member revocation query failed"
                );
                RosterError::GroupRevocation(failure.to_string())
            })
    }

    fn group_revocation_failure_kind(error: &KeyEpochError) -> &'static str {
        match error {
            KeyEpochError::Repository(_) => "repository",
            KeyEpochError::DecryptionFailed => "decryption",
            KeyEpochError::PersistedStateIntegrityFailed
            | KeyEpochError::InvalidRevocationStage
            | KeyEpochError::InvalidRevocationRecord => "integrity",
            KeyEpochError::SpaceNotReady
            | KeyEpochError::InvalidSpaceSecurityTransition { .. }
            | KeyEpochError::InvalidRevocationTransition { .. } => "state_transition",
            KeyEpochError::EpochOverflow
            | KeyEpochError::InvalidContentKeyId
            | KeyEpochError::ContentKeyReuse
            | KeyEpochError::RemovedMemberInOutbox
            | KeyEpochError::RevocationRecipientNotFound
            | KeyEpochError::PermanentLossRecipientNotPending
            | KeyEpochError::InvalidRevocationId => "validation",
        }
    }

    #[instrument(skip_all)]
    pub async fn secure_remove_legacy_member(
        &self,
        device_id: &str,
    ) -> Result<LegacyBootstrapView, RosterError> {
        let device_id = DeviceId::new(device_id);
        let (members, sponsor_device_id) = self.member_removal_context(&device_id).await?;
        let group_bootstrap = self
            .group_bootstrap
            .as_ref()
            .ok_or(RosterError::Unavailable)?;
        let retained_members = members
            .into_iter()
            .filter(|member| member.device_id != device_id)
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        let result = group_bootstrap
            .bootstrap_legacy_space(
                &sponsor_device_id,
                &retained_members,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| RosterError::GroupBootstrap(error.to_string()))?;
        let view = Self::legacy_bootstrap_view(result);
        if view.state != LegacyBootstrapState::RecoveryRequired {
            self.remove_member_records(&device_id).await?;
        }
        Ok(view)
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

    pub async fn query_revocation(
        &self,
        revocation_id: &str,
    ) -> Result<Option<MemberRevocationView>, RosterError> {
        let Some(group_revocation) = &self.group_revocation else {
            return Ok(None);
        };
        let revocation_id = uc_core::membership::RevocationId::from_string(revocation_id)
            .map_err(|error| RosterError::GroupRevocation(error.to_string()))?;
        let Some(revocation) = group_revocation
            .query_group_revocation(&revocation_id)
            .await
            .map_err(|error| RosterError::GroupRevocation(error.to_string()))?
        else {
            return Ok(None);
        };
        let revocation = self.retry_group_delivery(revocation).await?;
        Ok(Some(Self::revocation_view(revocation)))
    }

    pub async fn resume_incomplete_revocations(
        &self,
    ) -> Result<Vec<MemberRevocationView>, RosterError> {
        self.retry_pending_space_group_updates().await?;
        let Some(group_revocation) = &self.group_revocation else {
            return Ok(Vec::new());
        };
        let pending = group_revocation
            .resume_group_revocations(chrono::Utc::now().timestamp_millis())
            .await
            .map_err(|error| RosterError::GroupRevocation(error.to_string()))?;
        let mut results = Vec::with_capacity(pending.len());
        for revocation in pending {
            self.publish_member_revocation(Self::revocation_view(revocation.clone()));
            results.push(Self::revocation_view(
                self.retry_group_delivery(revocation).await?,
            ));
        }
        Ok(results)
    }

    pub async fn retry_pending_space_group_updates(&self) -> Result<usize, RosterError> {
        let Some(delivery) = &self.group_update_delivery else {
            return Ok(0);
        };
        delivery
            .deliver_pending(chrono::Utc::now().timestamp_millis())
            .await
            .map_err(|error| RosterError::GroupRevocation(error.to_string()))
    }

    async fn member_removal_context(
        &self,
        device_id: &DeviceId,
    ) -> Result<(Vec<SpaceMember>, DeviceId), RosterError> {
        let local_fingerprint = self.validate_remote_member(device_id).await?;
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?;
        let local_device_id = members
            .iter()
            .find(|member| member.identity_fingerprint == local_fingerprint)
            .map(|member| member.device_id.clone())
            .ok_or(RosterError::LocalMemberUnavailable)?;
        Ok((members, local_device_id))
    }

    async fn validate_remote_member(
        &self,
        device_id: &DeviceId,
    ) -> Result<uc_core::security::IdentityFingerprint, RosterError> {
        let member = self
            .member_repo
            .get(device_id)
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?
            .ok_or_else(|| RosterError::NotFound(device_id.as_str().to_string()))?;
        let local_fingerprint = self
            .local_identity
            .get_current_fingerprint()
            .await
            .map_err(|err| RosterError::LocalIdentity(err.to_string()))?
            .ok_or(RosterError::LocalMemberUnavailable)?;
        if member.identity_fingerprint == local_fingerprint {
            return Err(RosterError::LocalDeviceRemoval);
        }
        Ok(local_fingerprint)
    }

    async fn remove_member_records(&self, device_id: &DeviceId) -> Result<(), RosterError> {
        self.remove_peer_records(device_id, true).await
    }

    async fn remove_member_records_idempotent(
        &self,
        device_id: &DeviceId,
    ) -> Result<(), RosterError> {
        self.remove_peer_records(device_id, false).await
    }

    async fn remove_peer_records(
        &self,
        device_id: &DeviceId,
        missing_member_is_error: bool,
    ) -> Result<(), RosterError> {
        let removed = self
            .member_repo
            .remove(device_id)
            .await
            .map_err(|err| RosterError::MemberRepository(err.to_string()))?;
        if missing_member_is_error && !removed {
            return Err(RosterError::NotFound(device_id.as_str().to_string()));
        }
        self.peer_addr_repo
            .remove(device_id)
            .await
            .map_err(|err| RosterError::PeerAddressRepository(err.to_string()))?;
        self.trusted_peer_repo
            .remove(device_id)
            .await
            .map_err(|err| RosterError::TrustedPeerRepository(err.to_string()))?;
        Ok(())
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

    async fn retry_group_delivery(
        &self,
        mut revocation: GroupRevocationResult,
    ) -> Result<GroupRevocationResult, RosterError> {
        let (Some(group_revocation), Some(group_update_dispatch), Some(revocation_id)) = (
            &self.group_revocation,
            &self.group_update_dispatch,
            revocation.revocation_id().cloned(),
        ) else {
            return Ok(revocation);
        };
        let updates = group_revocation
            .pending_group_updates(&revocation_id)
            .await
            .map_err(|error| RosterError::GroupRevocation(error.to_string()))?;
        for update in updates {
            match group_update_dispatch.dispatch_group_update(&update).await {
                Ok(()) => {
                    revocation = group_revocation
                        .acknowledge_group_update(
                            &revocation_id,
                            update.recipient(),
                            chrono::Utc::now().timestamp_millis(),
                        )
                        .await
                        .map_err(|error| RosterError::GroupRevocation(error.to_string()))?;
                    self.publish_member_revocation(Self::revocation_view(revocation.clone()));
                }
                Err(_) => {
                    warn!(
                        has_revocation = update.revocation_id().is_some(),
                        "group update dispatch failed; delivery remains pending"
                    );
                }
            }
        }
        Ok(revocation)
    }

    fn publish_member_revocation(&self, revocation: MemberRevocationView) {
        let _ = self.member_revocation_events.send(revocation);
    }

    fn revocation_view(revocation: GroupRevocationResult) -> MemberRevocationView {
        match revocation {
            GroupRevocationResult::LocalOnly => MemberRevocationView {
                revocation_id: None,
                state: MemberRevocationState::LocalOnly,
                pending_recipients: 0,
                removed_device_ids: Vec::new(),
                pending_recipient_device_ids: Vec::new(),
                updated_at_ms: 0,
            },
            GroupRevocationResult::Reliable {
                revocation_id,
                status,
                removed_device_ids,
                pending_recipient_device_ids,
                updated_at_ms,
            } => MemberRevocationView {
                revocation_id: Some(revocation_id.as_str().to_owned()),
                state: match status {
                    RevocationStatus::Complete => MemberRevocationState::Complete,
                    RevocationStatus::RecoveryRequired => MemberRevocationState::RecoveryRequired,
                    RevocationStatus::Prepared
                    | RevocationStatus::Staged
                    | RevocationStatus::Activated
                    | RevocationStatus::Distributing => MemberRevocationState::Applied,
                },
                pending_recipients: pending_recipient_device_ids.len(),
                removed_device_ids: removed_device_ids
                    .into_iter()
                    .map(|device_id| device_id.as_str().to_owned())
                    .collect(),
                pending_recipient_device_ids: pending_recipient_device_ids
                    .into_iter()
                    .map(|device_id| device_id.as_str().to_owned())
                    .collect(),
                updated_at_ms,
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

#[cfg(test)]
mod tests {
    //! 单元测试围绕 7.1 验收点展开:
    //!
    //! * list_with_presence 聚合正确(成员数 / 顺序 / state 正确映射)
    //! * 本机标记(`is_local`:唯一匹配 fingerprint 的那条)
    //! * subscribe receiver 实时收事件
    //!
    //! 加上错误路径:member_repo / local_identity 故障能翻译成 `RosterError`。
    //!
    //! 并发性不是本 facade 关心点(`list_with_presence` 是串行 await,
    //! 顺序调 `current_state`)—— T6 已经专门覆盖 presence 并发路径。

    use super::*;

    #[test]
    fn member_revocation_logging_does_not_include_device_identifiers() {
        let source = include_str!("facade.rs");
        let production = source
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source must precede the test module");

        assert!(!production.contains("device_id = %"));
        assert!(!production.contains("recipient = %"));
        assert!(!production.contains("%device_id"));
    }

    #[test]
    fn current_member_revocation_failure_kinds_are_safe_and_specific() {
        assert_eq!(
            MemberRosterFacade::group_revocation_failure_kind(
                &uc_core::membership::KeyEpochError::Repository("private failure".into(),),
            ),
            "repository"
        );
        assert_eq!(
            MemberRosterFacade::group_revocation_failure_kind(
                &uc_core::membership::KeyEpochError::DecryptionFailed,
            ),
            "decryption"
        );
        assert_eq!(
            MemberRosterFacade::group_revocation_failure_kind(
                &uc_core::membership::KeyEpochError::PersistedStateIntegrityFailed,
            ),
            "integrity"
        );
        assert_eq!(
            MemberRosterFacade::group_revocation_failure_kind(
                &uc_core::membership::KeyEpochError::InvalidRevocationTransition {
                    from: RevocationStatus::Prepared,
                    to: RevocationStatus::Complete,
                },
            ),
            "state_transition"
        );
    }

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use std::sync::Mutex as StdMutex;

    use crate::facade::roster::{ContentTypesPatch, MemberSyncPreferencesPatch};
    use uc_core::ids::DeviceId;
    use uc_core::membership::{MemberSyncPreferences, MembershipError, SpaceMember};
    use uc_core::ports::peer_address::{PeerAddressError, PeerAddressRecord};
    use uc_core::ports::{
        ConnectionChannel, ConnectionChannelPort, ConnectionPath, LocalIdentityError,
        PresenceError, PresenceEvent, ReachabilityState,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::trusted_peer::{TrustedPeer, TrustedPeerError};

    // ── mockall: member_repo ────────────────────────────────────────────

    mockall::mock! {
        pub MemberRepo {}

        #[async_trait]
        impl MemberRepositoryPort for MemberRepo {
            async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError>;
            async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError>;
            async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError>;
            async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError>;
        }
    }

    // ── mockall: peer_addr_repo ─────────────────────────────────────────

    mockall::mock! {
        pub PeerAddrRepo {}

        #[async_trait]
        impl PeerAddressRepositoryPort for PeerAddrRepo {
            async fn get(&self, device: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError>;
            async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError>;
            async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError>;
            async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError>;
        }
    }

    // ── mockall: trusted_peer_repo ──────────────────────────────────────

    mockall::mock! {
        pub TrustedPeerRepo {}

        #[async_trait]
        impl TrustedPeerRepositoryPort for TrustedPeerRepo {
            async fn get(&self, peer_device_id: &DeviceId) -> Result<Option<TrustedPeer>, TrustedPeerError>;
            async fn list(&self) -> Result<Vec<TrustedPeer>, TrustedPeerError>;
            async fn save(&self, trusted_peer: &TrustedPeer) -> Result<(), TrustedPeerError>;
            async fn remove(&self, peer_device_id: &DeviceId) -> Result<bool, TrustedPeerError>;
        }
    }

    // ── mockall: local_identity ─────────────────────────────────────────

    mockall::mock! {
        pub LocalIdentity {}

        #[async_trait]
        impl LocalIdentityPort for LocalIdentity {
            async fn create(&self) -> Result<IdentityFingerprint, LocalIdentityError>;
            async fn ensure(&self) -> Result<IdentityFingerprint, LocalIdentityError>;
            async fn get_current_fingerprint(
                &self,
            ) -> Result<Option<IdentityFingerprint>, LocalIdentityError>;
        }
    }

    // ── hand-written fake: PresencePort ─────────────────────────────────
    //
    // 手写 fake 比 mockall 更适合这个场景:
    // 1. `current_state` 要按不同 device_id 返不同 state —— mockall 的
    //    `.withf(...).returning(...)` 每次得配一条 expectation,啰嗦。
    // 2. `subscribe` 要返 Receiver,Receiver 不 Clone,mockall 里配一次性
    //    返回值要借个 `Mutex<Option<..>>` 比较绕。
    // 3. subscribe 测试要 emit 一个事件给 receiver,需要直接持 Sender,
    //    fake 直接暴露 `emit(event)` 比通过 mockall 间接更清晰。

    struct FakePresence {
        states: StdMutex<Vec<(DeviceId, ReachabilityState)>>,
        tx: broadcast::Sender<PresenceEvent>,
    }

    impl FakePresence {
        fn new(entries: Vec<(DeviceId, ReachabilityState)>) -> Self {
            let (tx, _rx) = broadcast::channel(16);
            Self {
                states: StdMutex::new(entries),
                tx,
            }
        }
        fn emit(&self, event: PresenceEvent) {
            // 忽略无订阅时的 send 失败 —— 测试里只 emit 一次,调用前
            // 先拿了 receiver。
            let _ = self.tx.send(event);
        }
    }

    #[async_trait]
    impl PresencePort for FakePresence {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            unreachable!("MemberRosterFacade 不走 ensure_reachable 路径")
        }
        async fn current_state(&self, device: &DeviceId) -> ReachabilityState {
            self.states
                .lock()
                .unwrap()
                .iter()
                .find(|(d, _)| d == device)
                .map(|(_, s)| *s)
                .unwrap_or(ReachabilityState::Unknown)
        }
        fn subscribe(&self) -> broadcast::Receiver<PresenceEvent> {
            self.tx.subscribe()
        }
    }

    struct FakeConnectionChannel {
        paths: StdMutex<Vec<(DeviceId, ConnectionPath)>>,
    }

    impl FakeConnectionChannel {
        fn new(paths: Vec<(DeviceId, ConnectionPath)>) -> Self {
            Self {
                paths: StdMutex::new(paths),
            }
        }
    }

    #[async_trait]
    impl ConnectionChannelPort for FakeConnectionChannel {
        async fn path_for(&self, device: &DeviceId) -> ConnectionPath {
            self.paths
                .lock()
                .unwrap()
                .iter()
                .find(|(d, _)| d == device)
                .map(|(_, path)| path.clone())
                .unwrap_or_default()
        }
    }

    // ── helpers ─────────────────────────────────────────────────────────

    fn fp(seed: &str) -> IdentityFingerprint {
        // IdentityFingerprint 要求固定 16 字符 base32-like 字符串(见其
        // from_raw_string 校验)。本测试用固定 seed 组 + pad。
        let padded = format!("{:A<16}", seed)
            .chars()
            .take(16)
            .collect::<String>();
        IdentityFingerprint::from_raw_string(&padded).expect("测试 seed 要能通过 fingerprint 校验")
    }

    fn member(device: &str, name: &str, fingerprint: IdentityFingerprint) -> SpaceMember {
        SpaceMember {
            device_id: DeviceId::new(device),
            device_name: name.to_string(),
            identity_fingerprint: fingerprint,
            joined_at: Utc.with_ymd_and_hms(2026, 4, 21, 10, 0, 0).unwrap(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    fn build_facade(
        member_repo: MockMemberRepo,
        local_identity: MockLocalIdentity,
        presence: Arc<FakePresence>,
    ) -> MemberRosterFacade {
        // 多数测试不走 revoke_member 路径,默认给两个不设 expectation 的
        // mock —— mockall 严格模式下若意外被调用会 panic,等价于
        // "断言这两个 repo 在该测试中不应被触发"。
        build_facade_with_unpair_repos(
            member_repo,
            MockPeerAddrRepo::new(),
            MockTrustedPeerRepo::new(),
            local_identity,
            presence,
        )
    }

    fn build_facade_with_unpair_repos(
        member_repo: MockMemberRepo,
        peer_addr_repo: MockPeerAddrRepo,
        trusted_peer_repo: MockTrustedPeerRepo,
        local_identity: MockLocalIdentity,
        presence: Arc<FakePresence>,
    ) -> MemberRosterFacade {
        MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(member_repo),
            peer_addr_repo: Arc::new(peer_addr_repo),
            trusted_peer_repo: Arc::new(trusted_peer_repo),
            local_identity: Arc::new(local_identity),
            presence,
            connection_channel: None,
        })
    }

    fn expect_remote_member_lookup(
        repo: &mut MockMemberRepo,
        local_identity: &mut MockLocalIdentity,
        device_id: &str,
    ) {
        let expected_device_id = DeviceId::new(device_id);
        let remote_member = member(device_id, "Remote device", fp("REMOTE"));
        repo.expect_get()
            .times(1)
            .withf(move |candidate| candidate == &expected_device_id)
            .returning(move |_| Ok(Some(remote_member.clone())));
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(Some(fp("LOCAL"))));
    }

    // ── tests ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_with_presence_empty_roster_returns_empty_vec() {
        let mut repo = MockMemberRepo::new();
        repo.expect_list().times(1).returning(|| Ok(vec![]));
        let mut id = MockLocalIdentity::new();
        // 空 roster 也要读一次 local fingerprint —— 顺序不敏感但要发生
        id.expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(Some(fp("LOCAL"))));
        let presence = Arc::new(FakePresence::new(vec![]));

        let facade = build_facade(repo, id, presence);
        let entries = facade.list_with_presence().await.expect("ok");
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn list_with_presence_marks_exactly_the_local_member() {
        let local_fp = fp("LOCAL");
        let remote_fp = fp("REMOTE");
        let m_local = member("dev-local", "laptop", local_fp.clone());
        let m_remote = member("dev-remote", "phone", remote_fp.clone());

        let mut repo = MockMemberRepo::new();
        let members = vec![m_local.clone(), m_remote.clone()];
        repo.expect_list()
            .times(1)
            .returning(move || Ok(members.clone()));
        let mut id = MockLocalIdentity::new();
        id.expect_get_current_fingerprint()
            .times(1)
            .returning(move || Ok(Some(local_fp.clone())));
        let presence = Arc::new(FakePresence::new(vec![
            (DeviceId::new("dev-local"), ReachabilityState::Online),
            (DeviceId::new("dev-remote"), ReachabilityState::Offline),
        ]));

        let facade = build_facade(repo, id, presence);
        let entries = facade.list_with_presence().await.expect("ok");
        assert_eq!(entries.len(), 2);

        let local = entries
            .iter()
            .find(|e| e.device_id == DeviceId::new("dev-local"))
            .expect("local entry");
        let remote = entries
            .iter()
            .find(|e| e.device_id == DeviceId::new("dev-remote"))
            .expect("remote entry");

        assert!(local.is_local, "fingerprint 匹配的那条必须 is_local = true");
        assert_eq!(local.device_name, "laptop");
        assert_eq!(local.state, ReachabilityState::Online);

        assert!(
            !remote.is_local,
            "fingerprint 不匹配的那条 is_local = false"
        );
        assert_eq!(remote.device_name, "phone");
        assert_eq!(remote.state, ReachabilityState::Offline);
    }

    #[tokio::test]
    async fn list_with_presence_without_local_identity_marks_all_false() {
        // pre-A1 / pre-B2 防御路径:local_identity 返回 Ok(None),仍能
        // 正常返回 roster,所有 entry is_local = false。
        let m = member("dev-x", "box", fp("SOMEFP"));
        let mut repo = MockMemberRepo::new();
        let members = vec![m];
        repo.expect_list()
            .times(1)
            .returning(move || Ok(members.clone()));
        let mut id = MockLocalIdentity::new();
        id.expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(None));
        let presence = Arc::new(FakePresence::new(vec![(
            DeviceId::new("dev-x"),
            ReachabilityState::Unknown,
        )]));

        let facade = build_facade(repo, id, presence);
        let entries = facade.list_with_presence().await.expect("ok");
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_local);
        assert_eq!(entries[0].state, ReachabilityState::Unknown);
    }

    #[tokio::test]
    async fn list_with_presence_state_defaults_to_unknown_when_presence_has_no_entry() {
        // PresencePort 契约:从未 probed 的 device 返 Unknown。本测试确保
        // facade 直接把这个值透传进 RosterEntry,不做二次翻译。
        let m = member("dev-fresh", "new-one", fp("FRESHFP"));
        let mut repo = MockMemberRepo::new();
        let members = vec![m];
        repo.expect_list()
            .times(1)
            .returning(move || Ok(members.clone()));
        let mut id = MockLocalIdentity::new();
        id.expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(Some(fp("LOCAL"))));
        let presence = Arc::new(FakePresence::new(vec![])); // 无缓存

        let facade = build_facade(repo, id, presence);
        let entries = facade.list_with_presence().await.expect("ok");
        assert_eq!(entries[0].state, ReachabilityState::Unknown);
    }

    #[tokio::test]
    async fn list_peer_snapshots_includes_current_connection_address() {
        let local_fp = fp("LOCAL");
        let remote_fp = fp("REMOTE");
        let m_local = member("dev-local", "laptop", local_fp.clone());
        let m_remote = member("dev-remote", "fedora", remote_fp);

        let mut repo = MockMemberRepo::new();
        let members = vec![m_local, m_remote];
        repo.expect_list()
            .times(1)
            .returning(move || Ok(members.clone()));
        let mut id = MockLocalIdentity::new();
        id.expect_get_current_fingerprint()
            .times(1)
            .returning(move || Ok(Some(local_fp.clone())));
        let presence = Arc::new(FakePresence::new(vec![(
            DeviceId::new("dev-remote"),
            ReachabilityState::Online,
        )]));

        let facade = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(repo),
            peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
            trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
            local_identity: Arc::new(id),
            presence,
            connection_channel: Some(Arc::new(FakeConnectionChannel::new(vec![(
                DeviceId::new("dev-remote"),
                ConnectionPath {
                    channel: ConnectionChannel::Direct,
                    address: Some("100.117.177.15:44868".to_string()),
                },
            )]))),
        });

        let snapshots = facade.list_peer_snapshots().await.expect("ok");

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].peer_id, "dev-remote");
        assert_eq!(snapshots[0].channel, ConnectionChannel::Direct);
        assert_eq!(
            snapshots[0].connection_address.as_deref(),
            Some("100.117.177.15:44868")
        );
    }

    #[tokio::test]
    async fn list_with_presence_surfaces_member_repo_failure() {
        let mut repo = MockMemberRepo::new();
        repo.expect_list()
            .times(1)
            .returning(|| Err(MembershipError::Repository("sqlite down".into())));
        let mut id = MockLocalIdentity::new();
        id.expect_get_current_fingerprint().times(0);
        let presence = Arc::new(FakePresence::new(vec![]));

        let facade = build_facade(repo, id, presence);
        let err = facade.list_with_presence().await.unwrap_err();
        match err {
            RosterError::MemberRepository(msg) => {
                assert!(msg.contains("sqlite down"), "msg = {msg}");
            }
            other => panic!("expected MemberRepository variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_with_presence_surfaces_local_identity_failure() {
        let mut repo = MockMemberRepo::new();
        repo.expect_list().times(1).returning(|| Ok(vec![]));
        let mut id = MockLocalIdentity::new();
        id.expect_get_current_fingerprint()
            .times(1)
            .returning(|| Err(LocalIdentityError::Storage("keychain locked".into())));
        let presence = Arc::new(FakePresence::new(vec![]));

        let facade = build_facade(repo, id, presence);
        let err = facade.list_with_presence().await.unwrap_err();
        match err {
            RosterError::LocalIdentity(msg) => {
                assert!(msg.contains("keychain locked"), "msg = {msg}");
            }
            other => panic!("expected LocalIdentity variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_presence_events_delivers_events_through_facade() {
        // T7 验收点:subscribe receiver 实时收事件。拿 receiver → 通过
        // fake 的 emit 发事件 → 确认 receiver 能收到。
        let repo = MockMemberRepo::new(); // 本测试不查 list
        let id = MockLocalIdentity::new(); // 也不查 identity
        let presence = Arc::new(FakePresence::new(vec![]));

        let facade = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(repo),
            peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
            trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
            local_identity: Arc::new(id),
            presence: Arc::clone(&presence) as Arc<dyn PresencePort>,
            connection_channel: None,
        });

        let mut rx = facade.subscribe_presence_events();
        let expected = PresenceEvent {
            device_id: DeviceId::new("dev-x"),
            state: ReachabilityState::Online,
            at: Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap(),
        };
        presence.emit(expected.clone());

        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("事件在超时内到达")
            .expect("broadcast 成功 recv");
        assert_eq!(got.device_id, expected.device_id);
        assert_eq!(got.state, expected.state);
    }

    #[tokio::test]
    async fn subscribe_presence_events_hands_out_independent_receivers() {
        // broadcast 语义:每次 subscribe() 拿到独立 receiver,一次 emit 两
        // 个 receiver 都能各自收到。
        let repo = MockMemberRepo::new();
        let id = MockLocalIdentity::new();
        let presence = Arc::new(FakePresence::new(vec![]));
        let facade = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(repo),
            peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
            trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
            local_identity: Arc::new(id),
            presence: Arc::clone(&presence) as Arc<dyn PresencePort>,
            connection_channel: None,
        });

        let mut rx1 = facade.subscribe_presence_events();
        let mut rx2 = facade.subscribe_presence_events();
        presence.emit(PresenceEvent {
            device_id: DeviceId::new("d"),
            state: ReachabilityState::Online,
            at: Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap(),
        });

        let got1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx1.recv())
            .await
            .unwrap()
            .unwrap();
        let got2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx2.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got1.state, ReachabilityState::Online);
        assert_eq!(got2.state, ReachabilityState::Online);
    }

    #[tokio::test]
    async fn get_sync_preferences_accepts_application_device_id() {
        let mut repo = MockMemberRepo::new();
        let member = member("dev-1", "phone", fp("REMOTE"));
        let expected = member.sync_preferences.clone();
        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(member.clone())));
        let id = MockLocalIdentity::new();
        let presence = Arc::new(FakePresence::new(vec![]));
        let facade = build_facade(repo, id, presence);

        let got = facade.get_sync_preferences("dev-1").await.expect("ok");

        assert_eq!(got.send_enabled, expected.send_enabled);
        assert_eq!(got.receive_enabled, expected.receive_enabled);
        assert_eq!(
            got.send_content_types.text,
            expected.send_content_types.text
        );
    }

    #[tokio::test]
    async fn update_sync_preferences_patch_preserves_unmentioned_fields() {
        let mut repo = MockMemberRepo::new();
        let mut existing = member("dev-1", "phone", fp("REMOTE"));
        existing.sync_preferences.send_enabled = true;
        existing.sync_preferences.receive_enabled = true;
        existing.sync_preferences.send_content_types.text = false;
        existing.sync_preferences.send_content_types.image = true;
        let existing_for_get = existing.clone();

        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(existing_for_get.clone())));
        repo.expect_save()
            .times(1)
            .withf(|member| {
                member.device_id == DeviceId::new("dev-1")
                    && !member.sync_preferences.send_enabled
                    && member.sync_preferences.receive_enabled
                    && member.sync_preferences.send_content_types.text
                    && member.sync_preferences.send_content_types.image
            })
            .returning(|_| Ok(()));
        let id = MockLocalIdentity::new();
        let presence = Arc::new(FakePresence::new(vec![]));
        let facade = build_facade(repo, id, presence);

        let updated = facade
            .update_sync_preferences(
                "dev-1",
                MemberSyncPreferencesPatch {
                    send_enabled: Some(false),
                    receive_enabled: None,
                    send_content_types: Some(ContentTypesPatch {
                        text: Some(true),
                        image: None,
                        link: None,
                        file: None,
                        code_snippet: None,
                        rich_text: None,
                    }),
                    receive_content_types: None,
                },
            )
            .await
            .expect("ok");

        assert!(!updated.send_enabled);
        assert!(updated.receive_enabled);
        assert!(updated.send_content_types.text);
        assert!(updated.send_content_types.image);
    }

    #[tokio::test]
    async fn revoke_member_rejects_legacy_space_without_deleting_records() {
        let mut repo = MockMemberRepo::new();
        let mut id = MockLocalIdentity::new();
        expect_remote_member_lookup(&mut repo, &mut id, "dev-1");
        repo.expect_remove().times(0);
        let facade = build_facade_with_unpair_repos(
            repo,
            MockPeerAddrRepo::new(),
            MockTrustedPeerRepo::new(),
            id,
            Arc::new(FakePresence::new(vec![])),
        );

        let error = facade.revoke_member("dev-1").await.unwrap_err();

        assert!(matches!(error, RosterError::LegacyBootstrapRequired));
    }

    #[tokio::test]
    async fn revoke_member_rejects_the_local_device_before_deleting_any_records() {
        let local_fingerprint = fp("LOCAL");
        let local_member = member("dev-local", "This device", local_fingerprint.clone());
        let mut repo = MockMemberRepo::new();
        repo.expect_get()
            .times(1)
            .withf(|device_id| device_id == &DeviceId::new("dev-local"))
            .returning(move |_| Ok(Some(local_member.clone())));
        repo.expect_remove().times(0);
        let mut local_identity = MockLocalIdentity::new();
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(move || Ok(Some(local_fingerprint.clone())));
        let facade = build_facade_with_unpair_repos(
            repo,
            MockPeerAddrRepo::new(),
            MockTrustedPeerRepo::new(),
            local_identity,
            Arc::new(FakePresence::new(vec![])),
        );

        let result = facade.revoke_member("dev-local").await;

        assert!(
            result.is_err(),
            "the local device must leave instead of removing itself"
        );
    }

    mockall::mock! {
        GroupRevocation {}

        #[async_trait]
        impl uc_core::membership::GroupRevocationPort for GroupRevocation {
            async fn revoke_group_member(
                &self,
                target: &DeviceId,
                retained_recipients: &[DeviceId],
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupRevocationResult, uc_core::membership::KeyEpochError>;
            async fn acknowledge_group_update(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
                recipient: &DeviceId,
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupRevocationResult, uc_core::membership::KeyEpochError>;
            async fn apply_group_epoch_update(
                &self,
                payload: &[u8],
            ) -> Result<uc_core::membership::GroupEpoch, uc_core::membership::KeyEpochError>;
            async fn pending_group_updates(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
            ) -> Result<Vec<uc_core::membership::PendingGroupUpdate>, uc_core::membership::KeyEpochError>;
            async fn query_group_revocation(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
            ) -> Result<Option<uc_core::membership::GroupRevocationResult>, uc_core::membership::KeyEpochError>;
            async fn current_group_revocation(
                &self,
            ) -> Result<Option<uc_core::membership::GroupRevocationResult>, uc_core::membership::KeyEpochError>;
            async fn continue_group_revocation(
                &self,
                revocation_id: &uc_core::membership::RevocationId,
                permanently_lost_device_ids: &[DeviceId],
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupRevocationResult, uc_core::membership::KeyEpochError>;
            async fn resume_group_revocations(
                &self,
                now_ms: i64,
            ) -> Result<Vec<uc_core::membership::GroupRevocationResult>, uc_core::membership::KeyEpochError>;
            async fn pending_space_group_updates(
                &self,
            ) -> Result<Vec<uc_core::membership::PendingGroupUpdate>, uc_core::membership::KeyEpochError>;
            async fn acknowledge_space_group_update(
                &self,
                update_id: &str,
                now_ms: i64,
            ) -> Result<bool, uc_core::membership::KeyEpochError>;
        }
    }

    fn active_revocation(target: &str) -> uc_core::membership::GroupRevocationResult {
        uc_core::membership::GroupRevocationResult::Reliable {
            revocation_id: uc_core::membership::RevocationId::from_string("revocation-active")
                .unwrap(),
            status: uc_core::membership::RevocationStatus::Distributing,
            removed_device_ids: vec![DeviceId::new(target)],
            pending_recipient_device_ids: vec![DeviceId::new("dev-waiting")],
            updated_at_ms: 123,
        }
    }

    #[tokio::test]
    async fn repeated_removal_of_the_active_target_returns_existing_progress() {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(Some(active_revocation("dev-removed"))));
        group.expect_revoke_group_member().times(0);
        let mut repo = MockMemberRepo::new();
        repo.expect_get().times(0);
        repo.expect_list().times(0);
        repo.expect_remove().times(0);
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
                trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
                local_identity: Arc::new(MockLocalIdentity::new()),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
        );

        let result = facade.revoke_member("dev-removed").await.unwrap();

        assert_eq!(result.revocation_id.as_deref(), Some("revocation-active"));
        assert_eq!(result.state, MemberRevocationState::Applied);
        assert_eq!(result.pending_recipient_device_ids, ["dev-waiting"]);
    }

    #[tokio::test]
    async fn removal_of_another_target_while_one_is_active_returns_a_conflict() {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(Some(active_revocation("dev-removed"))));
        group.expect_revoke_group_member().times(0);
        let mut repo = MockMemberRepo::new();
        repo.expect_get().times(0);
        repo.expect_list().times(0);
        repo.expect_remove().times(0);
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
                trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
                local_identity: Arc::new(MockLocalIdentity::new()),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
        );

        let error = facade.revoke_member("dev-other").await.unwrap_err();

        assert!(matches!(error, RosterError::MemberRemovalInProgress));
    }

    #[tokio::test]
    async fn permanent_loss_recovery_rejects_a_device_that_is_not_waiting() {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(Some(active_revocation("dev-removed"))));
        group.expect_continue_group_revocation().times(0);
        let mut repo = MockMemberRepo::new();
        repo.expect_remove().times(0);
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
                trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
                local_identity: Arc::new(MockLocalIdentity::new()),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
        );

        let error = facade
            .continue_member_revocation("revocation-active", &["dev-not-waiting".to_owned()])
            .await
            .unwrap_err();

        assert!(matches!(error, RosterError::InvalidPermanentLossSelection));
    }

    #[tokio::test]
    async fn permanent_loss_recovery_advances_security_before_cleaning_records() {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(Some(active_revocation("dev-removed"))));
        group
            .expect_continue_group_revocation()
            .times(1)
            .withf(|revocation_id, lost, _| {
                revocation_id.as_str() == "revocation-active"
                    && lost == [DeviceId::new("dev-waiting")]
            })
            .returning(|revocation_id, _, _| {
                Ok(uc_core::membership::GroupRevocationResult::Reliable {
                    revocation_id: revocation_id.clone(),
                    status: uc_core::membership::RevocationStatus::Distributing,
                    removed_device_ids: vec![
                        DeviceId::new("dev-removed"),
                        DeviceId::new("dev-waiting"),
                    ],
                    pending_recipient_device_ids: vec![DeviceId::new("dev-retained")],
                    updated_at_ms: 200,
                })
            });
        let mut repo = MockMemberRepo::new();
        repo.expect_remove()
            .times(1)
            .withf(|device_id| device_id == &DeviceId::new("dev-waiting"))
            .returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(1).returning(|_| Ok(true));
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(MockLocalIdentity::new()),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
        );

        let result = facade
            .continue_member_revocation("revocation-active", &["dev-waiting".to_owned()])
            .await
            .unwrap();

        assert_eq!(result.removed_device_ids, ["dev-removed", "dev-waiting"]);
        assert_eq!(result.pending_recipient_device_ids, ["dev-retained"]);
    }

    #[tokio::test]
    async fn permanent_loss_recovery_accepts_already_removed_and_pending_devices_together() {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(Some(active_revocation("dev-removed"))));
        group
            .expect_continue_group_revocation()
            .times(1)
            .withf(|revocation_id, lost, _| {
                revocation_id.as_str() == "revocation-active"
                    && lost == [DeviceId::new("dev-waiting")]
            })
            .returning(|revocation_id, _, _| {
                Ok(uc_core::membership::GroupRevocationResult::Reliable {
                    revocation_id: revocation_id.clone(),
                    status: uc_core::membership::RevocationStatus::Distributing,
                    removed_device_ids: vec![
                        DeviceId::new("dev-removed"),
                        DeviceId::new("dev-waiting"),
                    ],
                    pending_recipient_device_ids: vec![DeviceId::new("dev-retained")],
                    updated_at_ms: 200,
                })
            });
        let mut repo = MockMemberRepo::new();
        repo.expect_remove().times(2).returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(2).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(2).returning(|_| Ok(true));
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(MockLocalIdentity::new()),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
        );

        let result = facade
            .continue_member_revocation(
                "revocation-active",
                &["dev-removed".to_owned(), "dev-waiting".to_owned()],
            )
            .await
            .unwrap();

        assert_eq!(result.removed_device_ids, ["dev-removed", "dev-waiting"]);
        assert_eq!(result.pending_recipient_device_ids, ["dev-retained"]);
    }

    mockall::mock! {
        GroupBootstrap {}

        #[async_trait]
        impl uc_core::membership::GroupBootstrapPort for GroupBootstrap {
            async fn bootstrap_legacy_space(
                &self,
                sponsor: &DeviceId,
                retained_members: &[DeviceId],
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>;
            async fn acknowledge_legacy_readmission(
                &self,
                bootstrap_id: &uc_core::membership::BootstrapId,
                member: &DeviceId,
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>;
            async fn withdraw_legacy_readmission(
                &self,
                bootstrap_id: &uc_core::membership::BootstrapId,
                member: &DeviceId,
                now_ms: i64,
            ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>;
            async fn query_legacy_bootstrap(
                &self,
                bootstrap_id: &uc_core::membership::BootstrapId,
            ) -> Result<Option<uc_core::membership::GroupBootstrapResult>, uc_core::membership::BootstrapError>;
            async fn resume_legacy_bootstraps(
                &self,
                now_ms: i64,
            ) -> Result<Vec<uc_core::membership::GroupBootstrapResult>, uc_core::membership::BootstrapError>;
        }
    }

    mockall::mock! {
        SpaceProtection {}

        #[async_trait]
        impl uc_core::membership::SpaceProtectionStatusPort for SpaceProtection {
            async fn query_space_protection(
                &self,
                members: &[DeviceId],
            ) -> Result<uc_core::membership::SpaceProtectionSnapshot, uc_core::membership::SpaceProtectionError>;
        }
    }

    mockall::mock! {
        GroupUpdateDispatch {}

        #[async_trait]
        impl uc_core::membership::GroupUpdateDispatchPort for GroupUpdateDispatch {
            async fn dispatch_group_update(
                &self,
                update: &uc_core::membership::PendingGroupUpdate,
            ) -> Result<(), uc_core::membership::GroupUpdateDispatchError>;
        }
    }

    fn completed_group_revocation() -> Arc<MockGroupRevocation> {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(None));
        group
            .expect_revoke_group_member()
            .times(1)
            .returning(|_, _, _| {
                Ok(uc_core::membership::GroupRevocationResult::Reliable {
                    revocation_id: uc_core::membership::RevocationId::from_string(
                        "revocation-complete",
                    )
                    .unwrap(),
                    status: uc_core::membership::RevocationStatus::Complete,
                    removed_device_ids: vec![DeviceId::new("dev-1")],
                    pending_recipient_device_ids: Vec::new(),
                    updated_at_ms: 100,
                })
            });
        Arc::new(group)
    }

    fn expect_ready_remote_removal(
        repo: &mut MockMemberRepo,
        local_identity: &mut MockLocalIdentity,
        device_id: &str,
    ) {
        expect_remote_member_lookup(repo, local_identity, device_id);
        let remote = member(device_id, "Remote device", fp("REMOTE"));
        let local = member("dev-local", "This device", fp("LOCAL"));
        repo.expect_list()
            .times(1)
            .returning(move || Ok(vec![local.clone(), remote.clone()]));
    }

    fn failing_group_revocation() -> Arc<MockGroupRevocation> {
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(None));
        group
            .expect_revoke_group_member()
            .times(1)
            .returning(|_, _, _| {
                Err(uc_core::membership::KeyEpochError::Repository(
                    "injected revocation failure".into(),
                ))
            });
        Arc::new(group)
    }

    fn reliable_group_delivery_mocks() -> (Arc<MockGroupRevocation>, Arc<MockGroupUpdateDispatch>) {
        let revocation_id = uc_core::membership::RevocationId::from_string("revocation-a").unwrap();
        let admission_update =
            uc_core::membership::PendingGroupUpdate::persistent(DeviceId::new("dev-2"), vec![0]);
        let admission_update_id = admission_update.update_id().to_string();
        let revocation_update = uc_core::membership::PendingGroupUpdate::new(
            revocation_id.clone(),
            DeviceId::new("dev-2"),
            vec![1, 2, 3],
        );

        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(None));
        let revoke_result_id = revocation_id.clone();
        group
            .expect_revoke_group_member()
            .times(1)
            .withf(|target, retained, _| {
                target == &DeviceId::new("dev-1") && retained == [DeviceId::new("dev-2")]
            })
            .returning(move |_, _, _| {
                Ok(uc_core::membership::GroupRevocationResult::Reliable {
                    revocation_id: revoke_result_id.clone(),
                    status: uc_core::membership::RevocationStatus::Distributing,
                    removed_device_ids: vec![DeviceId::new("dev-1")],
                    pending_recipient_device_ids: vec![DeviceId::new("dev-2")],
                    updated_at_ms: 100,
                })
            });
        group
            .expect_pending_space_group_updates()
            .times(1)
            .return_once(move || Ok(vec![admission_update]));
        group
            .expect_acknowledge_space_group_update()
            .times(1)
            .withf(move |update_id, _| update_id == admission_update_id)
            .returning(|_, _| Ok(true));
        let pending_id = revocation_id.clone();
        group
            .expect_pending_group_updates()
            .times(1)
            .withf(move |actual| actual == &pending_id)
            .return_once(move |_| Ok(vec![revocation_update]));
        group
            .expect_acknowledge_group_update()
            .times(1)
            .withf(move |actual_id, recipient, _| {
                actual_id == &revocation_id && recipient == &DeviceId::new("dev-2")
            })
            .returning(|actual_id, _, _| {
                Ok(uc_core::membership::GroupRevocationResult::Reliable {
                    revocation_id: actual_id.clone(),
                    status: uc_core::membership::RevocationStatus::Complete,
                    removed_device_ids: vec![DeviceId::new("dev-1")],
                    pending_recipient_device_ids: Vec::new(),
                    updated_at_ms: 101,
                })
            });

        let mut dispatch = MockGroupUpdateDispatch::new();
        let mut sequence = mockall::Sequence::new();
        dispatch
            .expect_dispatch_group_update()
            .times(1)
            .in_sequence(&mut sequence)
            .withf(|update| {
                update.recipient() == &DeviceId::new("dev-2") && update.payload() == [0]
            })
            .returning(|_| Ok(()));
        dispatch
            .expect_dispatch_group_update()
            .times(1)
            .in_sequence(&mut sequence)
            .withf(|update| {
                update.recipient() == &DeviceId::new("dev-2") && update.payload() == [1, 2, 3]
            })
            .returning(|_| Ok(()));

        (Arc::new(group), Arc::new(dispatch))
    }

    #[tokio::test]
    async fn revoke_member_keeps_all_records_when_reliable_revocation_fails() {
        let local_fingerprint = fp("LOCAL");
        let target = member("dev-1", "Remote device", fp("REMOTE"));
        let local = member("dev-local", "This device", local_fingerprint.clone());
        let mut repo = MockMemberRepo::new();
        let target_for_get = target.clone();
        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(target_for_get.clone())));
        repo.expect_list()
            .times(1)
            .returning(move || Ok(vec![local.clone(), target.clone()]));
        repo.expect_remove().times(0);
        let mut local_identity = MockLocalIdentity::new();
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(move || Ok(Some(local_fingerprint.clone())));
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(MockPeerAddrRepo::new()),
                trusted_peer_repo: Arc::new(MockTrustedPeerRepo::new()),
                local_identity: Arc::new(local_identity),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            failing_group_revocation(),
        );

        let error = facade.revoke_member("dev-1").await.unwrap_err();

        assert!(matches!(error, RosterError::GroupRevocation(_)));
    }

    #[tokio::test]
    async fn revoke_member_removes_a_legacy_peer_waiting_for_readmission() {
        let local_fingerprint = fp("LOCAL");
        let target = member("dev-1", "Legacy device", fp("REMOTE"));
        let local = member("dev-local", "This device", local_fingerprint.clone());
        let mut repo = MockMemberRepo::new();
        let target_for_get = target.clone();
        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(target_for_get.clone())));
        repo.expect_list()
            .times(1)
            .returning(move || Ok(vec![local.clone(), target.clone()]));
        repo.expect_remove()
            .times(1)
            .withf(|device_id| device_id == &DeviceId::new("dev-1"))
            .returning(|_| Ok(true));
        let mut local_identity = MockLocalIdentity::new();
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(move || Ok(Some(local_fingerprint.clone())));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(1).returning(|_| Ok(true));

        let bootstrap_id = BootstrapId::from_string("bootstrap-auto-upgrade").unwrap();
        let mut protection = MockSpaceProtection::new();
        let expected_bootstrap_id = bootstrap_id.clone();
        protection
            .expect_query_space_protection()
            .times(1)
            .withf(|members| members == [DeviceId::new("dev-local"), DeviceId::new("dev-1")])
            .returning(move |_| {
                Ok(SpaceProtectionSnapshot {
                    mode: CoreSpaceProtectionMode::Ready,
                    members: vec![uc_core::membership::MemberProtection {
                        device_id: DeviceId::new("dev-1"),
                        status: CoreMemberProtectionStatus::AwaitingReadmission,
                    }],
                    legacy_bootstrap: Some(uc_core::membership::LegacyBootstrapProgress {
                        bootstrap_id: expected_bootstrap_id.clone(),
                        status: LegacyBootstrapStatus::AwaitingReadmission,
                        pending_readmission: 1,
                    }),
                })
            });
        let mut bootstrap = MockGroupBootstrap::new();
        bootstrap
            .expect_withdraw_legacy_readmission()
            .times(1)
            .withf(move |actual_id, member, _| {
                actual_id == &bootstrap_id && member == &DeviceId::new("dev-1")
            })
            .returning(|actual_id, _, _| {
                Ok(GroupBootstrapResult::Complete {
                    bootstrap_id: actual_id.clone(),
                })
            });
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(None));
        group.expect_revoke_group_member().times(0);
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(local_identity),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
        )
        .with_group_bootstrap(Arc::new(bootstrap))
        .with_space_protection(Arc::new(protection));

        let result = facade.revoke_member("dev-1").await.unwrap();

        assert_eq!(result.state, MemberRevocationState::Complete);
        assert_eq!(result.pending_recipients, 0);
    }

    #[tokio::test]
    async fn secure_legacy_removal_bootstraps_before_cleaning_target_records() {
        let local = member("dev-local", "This device", fp("LOCAL"));
        let target = member("dev-1", "Removed device", fp("REMOTE"));
        let retained = member("dev-2", "Retained device", fp("RETAINED"));
        let mut repo = MockMemberRepo::new();
        let target_for_get = target.clone();
        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(target_for_get.clone())));
        repo.expect_list()
            .times(1)
            .returning(move || Ok(vec![local.clone(), target.clone(), retained.clone()]));
        repo.expect_remove()
            .times(1)
            .withf(|device_id| device_id == &DeviceId::new("dev-1"))
            .returning(|_| Ok(true));
        let mut local_identity = MockLocalIdentity::new();
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(Some(fp("LOCAL"))));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(1).returning(|_| Ok(true));
        let mut bootstrap = MockGroupBootstrap::new();
        bootstrap
            .expect_bootstrap_legacy_space()
            .times(1)
            .withf(|sponsor, retained_members, _| {
                sponsor == &DeviceId::new("dev-local")
                    && retained_members == [DeviceId::new("dev-local"), DeviceId::new("dev-2")]
            })
            .returning(|_, _, _| {
                Ok(GroupBootstrapResult::AwaitingReadmission {
                    bootstrap_id: BootstrapId::from_string("bootstrap-a").unwrap(),
                    pending_members: 1,
                })
            });
        let facade = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::new(repo),
            peer_addr_repo: Arc::new(peer_addr),
            trusted_peer_repo: Arc::new(trusted),
            local_identity: Arc::new(local_identity),
            presence: Arc::new(FakePresence::new(vec![])),
            connection_channel: None,
        })
        .with_group_bootstrap(Arc::new(bootstrap));

        let result = facade.secure_remove_legacy_member("dev-1").await.unwrap();

        assert_eq!(result.bootstrap_id, "bootstrap-a");
        assert_eq!(result.state, LegacyBootstrapState::AwaitingReadmission);
        assert_eq!(result.pending_readmission, 1);
    }

    #[tokio::test]
    async fn revoke_member_confirms_delivered_group_updates() {
        let (group, dispatch) = reliable_group_delivery_mocks();
        let mut repo = MockMemberRepo::new();
        let target = member("dev-1", "Remote device", fp("REMOTE"));
        let retained = member("dev-2", "Retained device", fp("RETAINED"));
        let local = member("dev-local", "This device", fp("LOCAL"));
        let target_for_get = target.clone();
        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(target_for_get.clone())));
        repo.expect_list()
            .times(1)
            .returning(move || Ok(vec![local.clone(), target.clone(), retained.clone()]));
        repo.expect_remove().times(1).returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(1).returning(|_| Ok(true));
        let mut local_identity = MockLocalIdentity::new();
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(Some(fp("LOCAL"))));
        let facade = MemberRosterFacade::new_with_group_delivery(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(local_identity),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            group.clone(),
            dispatch.clone(),
        );
        let mut revocation_events = facade.subscribe_member_revocation_events();

        let result = facade.revoke_member("dev-1").await.unwrap();

        assert_eq!(result.state, MemberRevocationState::Complete);
        assert_eq!(result.pending_recipients, 0);
        let applied = revocation_events.recv().await.unwrap();
        assert_eq!(applied.state, MemberRevocationState::Applied);
        assert_eq!(applied.pending_recipient_device_ids, ["dev-2"]);
        let complete = revocation_events.recv().await.unwrap();
        assert_eq!(complete.state, MemberRevocationState::Complete);
        assert!(complete.pending_recipient_device_ids.is_empty());
    }

    #[tokio::test]
    async fn revoke_member_cleans_up_before_deferred_delivery_failure() {
        let revocation_id =
            uc_core::membership::RevocationId::from_string("revocation-deferred").unwrap();
        let admission_update =
            uc_core::membership::PendingGroupUpdate::persistent(DeviceId::new("dev-2"), vec![0]);
        let mut group = MockGroupRevocation::new();
        group
            .expect_current_group_revocation()
            .times(1)
            .returning(|| Ok(None));
        group
            .expect_revoke_group_member()
            .times(1)
            .returning(move |_, _, _| {
                Ok(uc_core::membership::GroupRevocationResult::Reliable {
                    revocation_id: revocation_id.clone(),
                    status: uc_core::membership::RevocationStatus::Distributing,
                    removed_device_ids: vec![DeviceId::new("dev-1")],
                    pending_recipient_device_ids: vec![DeviceId::new("dev-2")],
                    updated_at_ms: 100,
                })
            });
        group
            .expect_pending_space_group_updates()
            .times(1)
            .return_once(move || Ok(vec![admission_update]));
        group
            .expect_acknowledge_space_group_update()
            .times(1)
            .returning(|_, _| {
                Err(uc_core::membership::KeyEpochError::Repository(
                    "deferred ack failure".into(),
                ))
            });
        group
            .expect_pending_group_updates()
            .times(1)
            .returning(|_| Ok(Vec::new()));
        let mut dispatch = MockGroupUpdateDispatch::new();
        dispatch
            .expect_dispatch_group_update()
            .times(1)
            .returning(|_| Ok(()));

        let mut repo = MockMemberRepo::new();
        let target = member("dev-1", "Remote device", fp("REMOTE"));
        let retained = member("dev-2", "Retained device", fp("RETAINED"));
        let local = member("dev-local", "This device", fp("LOCAL"));
        let target_for_get = target.clone();
        repo.expect_get()
            .times(1)
            .returning(move |_| Ok(Some(target_for_get.clone())));
        repo.expect_list()
            .times(1)
            .returning(move || Ok(vec![local.clone(), target.clone(), retained.clone()]));
        repo.expect_remove().times(1).returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(1).returning(|_| Ok(true));
        let mut local_identity = MockLocalIdentity::new();
        local_identity
            .expect_get_current_fingerprint()
            .times(1)
            .returning(|| Ok(Some(fp("LOCAL"))));
        let facade = MemberRosterFacade::new_with_group_delivery(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(local_identity),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            Arc::new(group),
            Arc::new(dispatch),
        );

        let result = facade.revoke_member("dev-1").await.unwrap();

        assert_eq!(result.state, MemberRevocationState::Applied);
        assert_eq!(result.pending_recipients, 1);
    }

    #[test]
    fn revocation_view_preserves_recovery_required() {
        let result = MemberRosterFacade::revocation_view(
            uc_core::membership::GroupRevocationResult::Reliable {
                revocation_id: uc_core::membership::RevocationId::from_string(
                    "revocation-recovery",
                )
                .unwrap(),
                status: uc_core::membership::RevocationStatus::RecoveryRequired,
                removed_device_ids: vec![DeviceId::new("dev-1")],
                pending_recipient_device_ids: vec![DeviceId::new("dev-2")],
                updated_at_ms: 100,
            },
        );

        assert_eq!(result.state, MemberRevocationState::RecoveryRequired);
    }

    #[tokio::test]
    async fn revoke_member_tolerates_missing_trusted_peer_row() {
        // peer 配对完成前/中途断流时,member_repo 已经写但 trusted_peer 还没
        // 写——unpair 时 trusted_peer.remove 返回 Ok(false)(不存在),应当
        // 视为正常完成,不应抛错。
        let mut repo = MockMemberRepo::new();
        let mut id = MockLocalIdentity::new();
        expect_ready_remote_removal(&mut repo, &mut id, "dev-1");
        repo.expect_remove().times(1).returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted.expect_remove().times(1).returning(|_| Ok(false));
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(id),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            completed_group_revocation(),
        );

        facade.revoke_member("dev-1").await.expect("ok");
    }

    #[tokio::test]
    async fn revoke_member_skips_cleanup_when_member_not_found() {
        // member_repo.remove 返回 false 表示成员不存在 —— facade 直接返回
        // NotFound,不应再去碰 peer_addr_repo / trusted_peer_repo。两个 mock
        // 都不设 expect_remove,被调用即 panic。
        let mut repo = MockMemberRepo::new();
        repo.expect_get().times(1).returning(|_| Ok(None));
        repo.expect_remove().times(0);
        let peer_addr = MockPeerAddrRepo::new();
        let trusted = MockTrustedPeerRepo::new();
        let id = MockLocalIdentity::new();
        let presence = Arc::new(FakePresence::new(vec![]));
        let facade = build_facade_with_unpair_repos(repo, peer_addr, trusted, id, presence);

        let err = facade.revoke_member("ghost").await.unwrap_err();
        assert!(matches!(err, RosterError::NotFound(d) if d == "ghost"));
    }

    #[tokio::test]
    async fn revoke_member_propagates_peer_addr_repo_failure() {
        // member 已经删了无法回滚 —— peer_addr 失败短路返回错误,
        // trusted_peer 也不应再被调用(短路,mock 不设 expect_remove)。
        let mut repo = MockMemberRepo::new();
        let mut id = MockLocalIdentity::new();
        expect_ready_remote_removal(&mut repo, &mut id, "dev-1");
        repo.expect_remove().times(1).returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr
            .expect_remove()
            .times(1)
            .returning(|_| Err(PeerAddressError::Internal("disk full".into())));
        let trusted = MockTrustedPeerRepo::new();
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(id),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            completed_group_revocation(),
        );

        let err = facade.revoke_member("dev-1").await.unwrap_err();
        match err {
            RosterError::PeerAddressRepository(msg) => {
                assert!(msg.contains("disk full"), "msg = {msg}");
            }
            other => panic!("expected PeerAddressRepository, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn revoke_member_propagates_trusted_peer_repo_failure() {
        // member + peer_addr 都成功后,trusted_peer 失败要冒泡 ——
        // UI 才能感知"trust 没清,已撤销设备仍被当可信对端",
        // 启动期 reconcile_trusted_peers 会在下次 boot 兜底。
        let mut repo = MockMemberRepo::new();
        let mut id = MockLocalIdentity::new();
        expect_ready_remote_removal(&mut repo, &mut id, "dev-1");
        repo.expect_remove().times(1).returning(|_| Ok(true));
        let mut peer_addr = MockPeerAddrRepo::new();
        peer_addr.expect_remove().times(1).returning(|_| Ok(()));
        let mut trusted = MockTrustedPeerRepo::new();
        trusted
            .expect_remove()
            .times(1)
            .returning(|_| Err(TrustedPeerError::Repository("io error".into())));
        let facade = MemberRosterFacade::new_with_group_revocation(
            MemberRosterDeps {
                member_repo: Arc::new(repo),
                peer_addr_repo: Arc::new(peer_addr),
                trusted_peer_repo: Arc::new(trusted),
                local_identity: Arc::new(id),
                presence: Arc::new(FakePresence::new(vec![])),
                connection_channel: None,
            },
            completed_group_revocation(),
        );

        let err = facade.revoke_member("dev-1").await.unwrap_err();
        match err {
            RosterError::TrustedPeerRepository(msg) => {
                assert!(msg.contains("io error"), "msg = {msg}");
            }
            other => panic!("expected TrustedPeerRepository, got {other:?}"),
        }
    }
}
