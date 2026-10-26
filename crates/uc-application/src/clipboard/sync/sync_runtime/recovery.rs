use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;
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
use crate::clipboard::sync::RecoverableDeliveryFeed;
use crate::deps::CurrentSpaceMemberScopePort;
use schedule::RetrySchedule;
use uc_observability_contract::{log_fields::log_id, uc_debug, uc_info, uc_warn, wire_enum};

mod schedule;
#[cfg(test)]
mod tests;

const RECOVERY_PAGE_SIZE: usize = 256;

wire_enum! {
    /// 一轮恢复为什么开始：固定词表。
    #[derive(Clone, Copy)]
    enum RecoveryTrigger: &'static str {
        /// 订阅可达性之后对已在线设备的启动扫描。
        Startup => "startup",
        /// 设备刚刚上线。
        PeerOnline => "peer_online",
        /// 设备仍在线，但此前留下了可恢复结果，按退避到期再试。
        Retry => "retry",
        /// 可恢复结果通知积压丢失，重新扫描当前在线设备。
        ResultsLagged => "results_lagged",
    }
}

wire_enum! {
    /// 一轮恢复的结局：每个提前返回都对应一个固定类别，不记录条目、设备或错误正文。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum RecoveryOutcome: &'static str {
        /// 向目标补发了一条待恢复内容，对端已确认收下（含重复确认）。
        Delivered => "delivered",
        /// 向目标补发了一条待恢复内容，但尚未得到确认；可恢复结果会再次排期。
        Dispatched => "dispatched",
        /// 没有需要恢复的条目。
        NothingToRecover => "nothing_to_recover",
        /// 恢复被停止信号打断。
        Cancelled => "cancelled",
        /// 自动同步已关闭。
        SyncDisabled => "sync_disabled",
        /// 本机已不是当前空间的有效成员。
        MemberInactive => "member_inactive",
        /// 读取当前成员范围失败。
        ScopeUnavailable => "scope_unavailable",
        /// 目标设备被暂时暂停，保留其待发送内容。
        TargetPaused => "target_paused",
        /// 目标已不在成员范围内，待发送内容被标记为已取代。
        TargetNotCurrent => "target_not_current",
        /// 列举条目失败，扫描中止。
        ListingFailed => "listing_failed",
        /// 无法让旧内容失效，因此不补发。
        SupersedeFailed => "supersede_failed",
        /// 内容的投递状态在扫描期间已变为终态。
        StatusChanged => "status_changed",
        /// 条目内容已不存在，自动恢复被终止。
        PayloadLost => "payload_lost",
        /// 条目当前不可重发（不是内容丢失）。
        NotResendable => "not_resendable",
        /// 补发时存储或派送失败。
        DeliveryFailed => "delivery_failed",
        /// 补发被拒绝的其他固定原因。
        DeliverySkipped => "delivery_skipped",
    }
}

/// 一轮恢复结束后，按目标退避应如何继续。
enum RetryAfterRound {
    /// 已送达、进入终态或不再适合自动恢复：清除该目标的退避。
    Clear,
    /// 恢复轮次自身失败且没有落下新的投递结果：按退避再试。
    Again,
    /// 补发已经发出：若结果可恢复，记录负责人落库后会通知再次排期。
    AwaitResult,
}

impl RecoveryOutcome {
    fn retry_after_round(self) -> RetryAfterRound {
        match self {
            Self::Dispatched => RetryAfterRound::AwaitResult,
            Self::ListingFailed
            | Self::SupersedeFailed
            | Self::ScopeUnavailable
            | Self::DeliveryFailed => RetryAfterRound::Again,
            Self::Delivered
            | Self::NothingToRecover
            | Self::Cancelled
            | Self::SyncDisabled
            | Self::MemberInactive
            | Self::TargetPaused
            | Self::TargetNotCurrent
            | Self::StatusChanged
            | Self::PayloadLost
            | Self::NotResendable
            | Self::DeliverySkipped => RetryAfterRound::Clear,
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
    pub(super) recoverable_deliveries: RecoverableDeliveryFeed,
    pub(super) delivery_gate: Arc<tokio::sync::Mutex<()>>,
}

/// 只恢复已记录为暂时不可达的发送。触发来源有三类：启动扫描、设备上线事件，
/// 以及设备保持在线期间的可恢复结果（按目标有界退避重试，不依赖新的上线事件）。
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
        // 先订阅再扫描，扫描期间落库的结果不会丢失通知。
        let mut events = deps.peer_reachability.subscribe();
        let mut results = deps.recoverable_deliveries.subscribe();
        let task = tokio::spawn(async move {
            let mut schedule = RetrySchedule::default();
            recover_currently_online(
                &task_deps,
                RecoveryTrigger::Startup,
                &mut schedule,
                &task_cancel,
            )
            .await;
            loop {
                let next_due = schedule.next_due();
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => return,
                    event = events.recv() => match event {
                        Ok(event) if event.state == ReachabilityState::Online => {
                            // 上线事件立即恢复，此前的退避不再适用。
                            schedule.clear(&event.device_id);
                            recover_and_schedule(
                                &task_deps,
                                event.device_id,
                                RecoveryTrigger::PeerOnline,
                                &mut schedule,
                                &task_cancel,
                            )
                            .await;
                        }
                        Ok(event) if event.state == ReachabilityState::Offline => {
                            // 离线目标等待下一次上线事件，不空转重试。
                            schedule.clear(&event.device_id);
                        }
                        Ok(_) => {}
                        Err(RecvError::Lagged(missed)) => {
                            uc_warn!(missed = missed, "clipboard delivery recovery: presence events lagged");
                        }
                        Err(RecvError::Closed) => return,
                    },
                    result = results.recv() => match result {
                        Ok(result) => schedule.record_recoverable(
                            result.target_device_id,
                            Some(result.entry_id),
                            Instant::now(),
                        ),
                        Err(RecvError::Lagged(missed)) => {
                            uc_warn!(missed = missed, "clipboard delivery recovery: delivery results lagged");
                            recover_currently_online(
                                &task_deps,
                                RecoveryTrigger::ResultsLagged,
                                &mut schedule,
                                &task_cancel,
                            )
                            .await;
                        }
                        Err(RecvError::Closed) => return,
                    },
                    _ = tokio::time::sleep_until(next_due.unwrap_or_else(Instant::now)), if next_due.is_some() => {
                        if let Some(target) = schedule.take_due(Instant::now()) {
                            retry_due_target(&task_deps, target, &mut schedule, &task_cancel).await;
                        }
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

async fn recover_currently_online(
    deps: &OfflineDeliveryRecoveryDeps,
    trigger: RecoveryTrigger,
    schedule: &mut RetrySchedule,
    cancel: &CancellationToken,
) {
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
            schedule.clear(&peer.device_id);
            recover_and_schedule(deps, peer.device_id, trigger, schedule, cancel).await;
        }
    }
}

/// 退避到期的目标只在仍然在线时重试；否则交给下一次上线事件，避免为离线目标空转。
async fn retry_due_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    schedule: &mut RetrySchedule,
    cancel: &CancellationToken,
) {
    if deps.peer_reachability.current_state(&target).await != ReachabilityState::Online {
        schedule.clear(&target);
        return;
    }
    recover_and_schedule(deps, target, RecoveryTrigger::Retry, schedule, cancel).await;
}

/// 执行一轮恢复，并按结局决定该目标的退避：送达或终态清除，自身失败再排期，
/// 已发出的补发等待记录负责人发布的可恢复结果。
async fn recover_and_schedule(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    trigger: RecoveryTrigger,
    schedule: &mut RetrySchedule,
    cancel: &CancellationToken,
) {
    match recover_online_target(deps, target, trigger, cancel)
        .await
        .retry_after_round()
    {
        RetryAfterRound::Clear => schedule.clear(&target),
        RetryAfterRound::Again => schedule.record_recoverable(target, None, Instant::now()),
        RetryAfterRound::AwaitResult => {}
    }
}

/// 一轮恢复的完整动作：触发原因由调用方给出，结局由 `recover_for_target` 返回，这里写唯一的一条记录。
async fn recover_online_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    trigger: RecoveryTrigger,
    cancel: &CancellationToken,
) -> RecoveryOutcome {
    let _gate = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            record_recovery(trigger, RecoveryOutcome::Cancelled);
            return RecoveryOutcome::Cancelled;
        }
        gate = deps.delivery_gate.lock() => gate,
    };
    let outcome = recover_for_target(deps, target, cancel).await;
    record_recovery(trigger, outcome);
    outcome
}

fn record_recovery(trigger: RecoveryTrigger, outcome: RecoveryOutcome) {
    uc_info!(
        trigger = trigger.wire(),
        outcome = outcome.wire(),
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
                    if report.accepted + report.duplicate > 0 {
                        RecoveryOutcome::Delivered
                    } else {
                        RecoveryOutcome::Dispatched
                    }
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
