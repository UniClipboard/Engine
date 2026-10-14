use std::sync::Arc;

use async_trait::async_trait;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

use uc_core::clipboard::{DeliveryFailureReason, EntryDeliveryRecord, EntryDeliveryStatus};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::clipboard::{ClipboardEventRepositoryPort, ListClipboardEntriesPort};
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, EntryDeliveryRepositoryPort, PeerAddressRepositoryPort,
    PeerReachabilityPort, ReachabilityState, SettingsPort,
};

use super::automatic_sync_enabled;
use crate::clipboard::outbound::{
    ClipboardOutboundFacade, NotResendableReason, ResendEntryError, ResendReport,
};
use crate::deps::CurrentSpaceMemberScopePort;
use uc_observability_contract::{log_fields::log_id, uc_debug, uc_info, uc_warn};

#[cfg(test)]
mod tests;

const RECOVERY_PAGE_SIZE: usize = 256;

/// 一轮恢复为什么开始：固定词表。
#[derive(Clone, Copy)]
enum RecoveryTrigger {
    /// 订阅可达性之后对已在线设备的启动扫描。
    Startup,
    /// 设备刚刚上线。
    PeerOnline,
}

impl RecoveryTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::PeerOnline => "peer_online",
        }
    }
}

/// 一轮恢复的结局：每个提前返回都对应一个固定类别，不记录条目、设备或错误正文。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RecoveryOutcome {
    /// 向目标补发了一条待恢复内容，并拿到派送汇总。
    Dispatched,
    /// 没有需要恢复的条目。
    NothingToRecover,
    /// 恢复被停止信号打断。
    Cancelled,
    /// 自动同步已关闭。
    SyncDisabled,
    /// 本机已不是当前空间的有效成员。
    MemberInactive,
    /// 读取当前成员范围失败。
    ScopeUnavailable,
    /// 目标设备被暂时暂停，保留其待发送内容。
    TargetPaused,
    /// 目标已不在成员范围内，待发送内容被标记为已取代。
    TargetNotCurrent,
    /// 列举条目失败，扫描中止。
    ListingFailed,
    /// 无法让旧内容失效，因此不补发。
    SupersedeFailed,
    /// 内容的投递状态在扫描期间已变为终态。
    StatusChanged,
    /// 条目内容已不存在，自动恢复被终止。
    PayloadLost,
    /// 条目当前不可重发（不是内容丢失）。
    NotResendable,
    /// 补发时存储或派送失败。
    DeliveryFailed,
    /// 补发被拒绝的其他固定原因。
    DeliverySkipped,
}

impl RecoveryOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::NothingToRecover => "nothing_to_recover",
            Self::Cancelled => "cancelled",
            Self::SyncDisabled => "sync_disabled",
            Self::MemberInactive => "member_inactive",
            Self::ScopeUnavailable => "scope_unavailable",
            Self::TargetPaused => "target_paused",
            Self::TargetNotCurrent => "target_not_current",
            Self::ListingFailed => "listing_failed",
            Self::SupersedeFailed => "supersede_failed",
            Self::StatusChanged => "status_changed",
            Self::PayloadLost => "payload_lost",
            Self::NotResendable => "not_resendable",
            Self::DeliveryFailed => "delivery_failed",
            Self::DeliverySkipped => "delivery_skipped",
        }
    }
}

fn is_recovery_eligible(record: &EntryDeliveryRecord, target: &DeviceId) -> bool {
    record.target_device_id == *target
        && matches!(
            record.status,
            EntryDeliveryStatus::Pending | EntryDeliveryStatus::Unreachable
        )
}

#[async_trait]
pub(super) trait RecoveryDeliveryPort: Send + Sync {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError>;
}

#[async_trait]
impl RecoveryDeliveryPort for ClipboardOutboundFacade {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        ClipboardOutboundFacade::deliver_existing_local_entry(self, entry_id, targets).await
    }
}

pub(super) struct OfflineDeliveryRecoveryDeps {
    pub(super) peer_reachability: Arc<dyn PeerReachabilityPort>,
    pub(super) known_peers: Arc<dyn PeerAddressRepositoryPort>,
    pub(super) member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub(super) settings: Arc<dyn SettingsPort>,
    pub(super) entries: Arc<dyn ListClipboardEntriesPort>,
    pub(super) events: Arc<dyn ClipboardEventRepositoryPort>,
    pub(super) deliveries: Arc<dyn EntryDeliveryRepositoryPort>,
    pub(super) device_identity: Arc<dyn DeviceIdentityPort>,
    pub(super) clock: Arc<dyn ClockPort>,
    pub(super) delivery: Arc<dyn RecoveryDeliveryPort>,
    pub(super) delivery_gate: Arc<tokio::sync::Mutex<()>>,
}

/// 监听可达性变化，只恢复已记录为暂时不可达的发送。
pub(super) struct OfflineDeliveryRecovery {
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    deps: Arc<OfflineDeliveryRecoveryDeps>,
}

impl OfflineDeliveryRecovery {
    pub(super) fn start(deps: OfflineDeliveryRecoveryDeps) -> Self {
        let deps = Arc::new(deps);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_deps = Arc::clone(&deps);
        let task = tokio::spawn(async move {
            let mut events = task_deps.peer_reachability.subscribe();
            recover_currently_online(&task_deps, &task_cancel).await;
            loop {
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => return,
                    event = events.recv() => match event {
                        Ok(event) if event.state == ReachabilityState::Online => {
                            recover_online_target(
                                &task_deps,
                                event.device_id,
                                RecoveryTrigger::PeerOnline,
                                &task_cancel,
                            )
                            .await;
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            uc_warn!(missed = missed, "clipboard delivery recovery: presence events lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        });
        Self {
            cancel,
            task: tokio::sync::Mutex::new(Some(task)),
            deps,
        }
    }

    pub(super) async fn supersede_older_recoverable_entries(
        &self,
        entry_id: &EntryId,
        targets: &[DeviceId],
    ) {
        for target in targets {
            if !supersede_older_recoverable_entries(&self.deps, entry_id, target).await {
                uc_warn!(
                    entry_id = log_id(&entry_id),
                    "clipboard delivery recovery: unable to replace older offline content"
                );
            }
        }
    }

    pub(super) async fn shutdown(&self) -> Result<(), JoinError> {
        self.cancel.cancel();
        let mut task = self.task.lock().await;
        let result = match task.as_mut() {
            Some(task) => task.await,
            None => Ok(()),
        };
        *task = None;
        result
    }
}

impl Drop for OfflineDeliveryRecovery {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn recover_currently_online(deps: &OfflineDeliveryRecoveryDeps, cancel: &CancellationToken) {
    if cancel.is_cancelled() {
        return;
    }
    let peers = match deps.known_peers.list().await {
        Ok(peers) => peers,
        Err(_) => {
            uc_warn!(
                error_kind = "peer_listing",
                "clipboard delivery recovery: startup scan skipped"
            );
            return;
        }
    };
    for peer in peers {
        if cancel.is_cancelled() {
            return;
        }
        if deps.peer_reachability.current_state(&peer.device_id).await == ReachabilityState::Online
        {
            recover_online_target(deps, peer.device_id, RecoveryTrigger::Startup, cancel).await;
        }
    }
}

/// 一轮恢复的完整动作：触发原因由调用方给出，结局由 `recover_for_target` 返回，这里写唯一的一条记录。
async fn recover_online_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    trigger: RecoveryTrigger,
    cancel: &CancellationToken,
) {
    let _gate = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            record_recovery(trigger, RecoveryOutcome::Cancelled);
            return;
        }
        gate = deps.delivery_gate.lock() => gate,
    };
    let outcome = recover_for_target(deps, target, cancel).await;
    record_recovery(trigger, outcome);
}

fn record_recovery(trigger: RecoveryTrigger, outcome: RecoveryOutcome) {
    uc_info!(
        trigger = trigger.as_str(),
        outcome = outcome.as_str(),
        "clipboard delivery recovery round finished"
    );
}

async fn recover_for_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    cancel: &CancellationToken,
) -> RecoveryOutcome {
    if cancel.is_cancelled() {
        return RecoveryOutcome::Cancelled;
    }
    if !automatic_sync_enabled(deps.settings.as_ref()).await {
        return RecoveryOutcome::SyncDisabled;
    }

    let scope = match deps.member_scope.snapshot().await {
        Ok(scope) if scope.local_member_active => scope,
        Ok(_) => return RecoveryOutcome::MemberInactive,
        Err(_) => return RecoveryOutcome::ScopeUnavailable,
    };
    let target_is_current = scope.usable_peer_device_ids.contains(&target);
    if !target_is_current
        && scope
            .paused_peer_devices
            .iter()
            .any(|peer| peer.device_id == target)
    {
        return RecoveryOutcome::TargetPaused;
    }

    let local_device = deps.device_identity.current_device_id();
    let mut offset = 0;
    loop {
        if cancel.is_cancelled() {
            return RecoveryOutcome::Cancelled;
        }
        let entries = match deps.entries.list_entries(RECOVERY_PAGE_SIZE, offset).await {
            Ok(entries) => entries,
            Err(_) => {
                uc_warn!(
                    error_kind = "entry_listing",
                    "clipboard delivery recovery: scan stopped"
                );
                return RecoveryOutcome::ListingFailed;
            }
        };
        if entries.is_empty() {
            return RecoveryOutcome::NothingToRecover;
        }
        let count = entries.len();
        for entry in entries {
            if cancel.is_cancelled() {
                return RecoveryOutcome::Cancelled;
            }
            if !entry.delivery_tracked {
                continue;
            }
            let source = match deps.events.get_source_device(&entry.event_id).await {
                Ok(source) => source,
                Err(_) => {
                    uc_warn!(
                        error_kind = "entry_source",
                        "clipboard delivery recovery: entry skipped"
                    );
                    continue;
                }
            };
            if cancel.is_cancelled() {
                return RecoveryOutcome::Cancelled;
            }
            if source.as_ref() != Some(&local_device) {
                continue;
            }
            let records = match deps.deliveries.list_by_entry(&entry.entry_id).await {
                Ok(records) => records,
                Err(_) => {
                    uc_warn!(
                        error_kind = "delivery_lookup",
                        "clipboard delivery recovery: entry skipped"
                    );
                    continue;
                }
            };
            if cancel.is_cancelled() {
                return RecoveryOutcome::Cancelled;
            }
            let Some(record) = records
                .iter()
                .find(|record| record.target_device_id == target)
            else {
                continue;
            };
            if !target_is_current {
                supersede_delivery(deps, &entry.entry_id, &target).await;
                return RecoveryOutcome::TargetNotCurrent;
            }
            // 旧内容失效是一项完整动作，必须完成后才处理停止，避免重启后补发旧内容。
            if !supersede_older_recoverable_entries(deps, &entry.entry_id, &target).await {
                uc_warn!(
                    entry_id = log_id(&entry.entry_id),
                    "clipboard delivery recovery: unable to replace older offline content"
                );
                return RecoveryOutcome::SupersedeFailed;
            }
            if cancel.is_cancelled() {
                return RecoveryOutcome::Cancelled;
            }
            if !matches!(
                record.status,
                EntryDeliveryStatus::Pending | EntryDeliveryStatus::Unreachable
            ) {
                return RecoveryOutcome::StatusChanged;
            }
            if !automatic_sync_enabled(deps.settings.as_ref()).await {
                return RecoveryOutcome::SyncDisabled;
            }
            if cancel.is_cancelled() {
                return RecoveryOutcome::Cancelled;
            }
            return match deps
                .delivery
                .deliver_existing_local_entry(entry.entry_id.clone(), vec![target.clone()])
                .await
            {
                Ok(report) => {
                    uc_info!(
                        entry_id = log_id(&entry.entry_id),
                        accepted = report.accepted,
                        duplicate = report.duplicate,
                        offline = report.offline,
                        errored = report.errored,
                        pending = report.pending,
                        "clipboard delivery recovery dispatched"
                    );
                    RecoveryOutcome::Dispatched
                }
                Err(ResendEntryError::EntryNotResendable { reason, .. }) => {
                    if matches!(reason, NotResendableReason::PayloadLost) {
                        uc_info!(
                            entry_id = log_id(&entry.entry_id),
                            reason = "payload_lost",
                            "clipboard delivery recovery stopped: entry payload is gone"
                        );
                        stop_automatic_recovery(deps, &entry.entry_id, &target).await;
                        RecoveryOutcome::PayloadLost
                    } else {
                        RecoveryOutcome::NotResendable
                    }
                }
                Err(error @ (ResendEntryError::Storage(_) | ResendEntryError::Dispatch(_))) => {
                    uc_warn!(
                        error_kind = "delivery",
                        reason = error.kind(),
                        entry_id = log_id(&entry.entry_id),
                        "clipboard delivery recovery skipped entry"
                    );
                    RecoveryOutcome::DeliveryFailed
                }
                Err(error) => {
                    uc_debug!(
                        error_kind = "delivery",
                        reason = error.kind(),
                        entry_id = log_id(&entry.entry_id),
                        "clipboard delivery recovery skipped entry"
                    );
                    RecoveryOutcome::DeliverySkipped
                }
            };
        }
        if count < RECOVERY_PAGE_SIZE {
            return RecoveryOutcome::NothingToRecover;
        }
        offset += count;
    }
}

/// 仅保留当前条目向目标自动发送的资格。
/// 列表按新到旧排列，当前条目之后的旧内容不再允许自动发送。
async fn supersede_older_recoverable_entries(
    deps: &OfflineDeliveryRecoveryDeps,
    entry_id: &EntryId,
    target: &DeviceId,
) -> bool {
    let local_device = deps.device_identity.current_device_id();
    let mut offset = 0;
    let mut found_current = false;
    loop {
        let entries = match deps.entries.list_entries(RECOVERY_PAGE_SIZE, offset).await {
            Ok(entries) => entries,
            Err(_) => return false,
        };
        if entries.is_empty() {
            return found_current;
        }
        let count = entries.len();
        for entry in entries {
            if !found_current {
                if entry.entry_id == *entry_id {
                    found_current = true;
                }
                continue;
            }
            if !entry.delivery_tracked {
                continue;
            }
            let source = match deps.events.get_source_device(&entry.event_id).await {
                Ok(source) => source,
                Err(_) => continue,
            };
            if source.as_ref() != Some(&local_device) {
                continue;
            }
            let records = match deps.deliveries.list_by_entry(&entry.entry_id).await {
                Ok(records) => records,
                Err(_) => return false,
            };
            if !records
                .iter()
                .any(|record| is_recovery_eligible(record, target))
            {
                continue;
            }
            let superseded = EntryDeliveryRecord {
                entry_id: entry.entry_id,
                target_device_id: target.clone(),
                status: EntryDeliveryStatus::Superseded,
                reason_detail: None,
                updated_at_ms: deps.clock.now_ms(),
            };
            if deps.deliveries.record_attempt(&superseded).await.is_err() {
                return false;
            }
        }
        if count < RECOVERY_PAGE_SIZE {
            return found_current;
        }
        offset += count;
    }
}

async fn stop_automatic_recovery(
    deps: &OfflineDeliveryRecoveryDeps,
    entry_id: &EntryId,
    target: &DeviceId,
) {
    let record = EntryDeliveryRecord {
        entry_id: entry_id.clone(),
        target_device_id: target.clone(),
        status: EntryDeliveryStatus::Failed {
            reason: DeliveryFailureReason::Internal,
        },
        reason_detail: None,
        updated_at_ms: deps.clock.now_ms(),
    };
    if deps.deliveries.record_attempt(&record).await.is_err() {
        uc_warn!(
            error_kind = "delivery_record",
            "clipboard delivery recovery: failed to stop unavailable entry recovery"
        );
    }
}

async fn supersede_delivery(
    deps: &OfflineDeliveryRecoveryDeps,
    entry_id: &EntryId,
    target: &DeviceId,
) {
    let record = EntryDeliveryRecord {
        entry_id: entry_id.clone(),
        target_device_id: target.clone(),
        status: EntryDeliveryStatus::Superseded,
        reason_detail: None,
        updated_at_ms: deps.clock.now_ms(),
    };
    if deps.deliveries.record_attempt(&record).await.is_err() {
        uc_warn!(
            error_kind = "delivery_record",
            "clipboard delivery recovery: failed to stop ineligible target recovery"
        );
    }
}
