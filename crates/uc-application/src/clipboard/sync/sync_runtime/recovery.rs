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
use crate::clipboard::sync::{DeliveryResult, DeliveryResultFeed, DeliveryResultKind};
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
        /// 设备仍在线，但此前留下了可恢复结果，按退避到期再试；同一条目最多 3 次。
        Retry => "retry",
        /// 可恢复结果通知积压丢失，重新扫描当前在线设备。
        ResultsLagged => "results_lagged",
    }
}

wire_enum! {
    /// 一轮恢复的结局：每个提前返回都对应一个固定类别，不记录条目、设备或错误正文。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum RecoveryOutcome: &'static str {
        /// 向目标补发了一条待恢复内容，对端已确认收下（含重复确认）。确认不等于结算：
        /// 退避仍等待记录负责人的结果通知，最终写入失败时按同一条目的次数继续。
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
    /// 没有发出补发，且已进入终态或不再适合自动恢复：清除该目标的退避。
    Clear,
    /// 恢复轮次自身失败且没有落下新的投递结果：按退避再试。
    Again,
    /// 补发已经发出（无论对端是否已确认）：记录负责人在每次写入结束后通知结果，
    /// 已结算通知是清除退避的唯一依据；可恢复或未能落库时沿用同一条目已用的次数再次排期。
    AwaitResult,
}

/// 一轮恢复的结局，以及本轮已补发、仍在等待结果通知的条目。
struct RecoveryRound {
    outcome: RecoveryOutcome,
    awaiting: Option<EntryId>,
}

impl From<RecoveryOutcome> for RecoveryRound {
    fn from(outcome: RecoveryOutcome) -> Self {
        Self {
            outcome,
            awaiting: None,
        }
    }
}

impl RecoveryOutcome {
    fn retry_after_round(self) -> RetryAfterRound {
        match self {
            Self::Delivered | Self::Dispatched => RetryAfterRound::AwaitResult,
            Self::ListingFailed
            | Self::SupersedeFailed
            | Self::ScopeUnavailable
            | Self::DeliveryFailed => RetryAfterRound::Again,
            Self::NothingToRecover
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
    pub(super) delivery_results: DeliveryResultFeed,
    pub(super) delivery_gate: Arc<tokio::sync::Mutex<()>>,
}

/// 只恢复持久状态仍未结束（`Pending`/`Unreachable`）的发送。触发来源有三类：启动扫描、
/// 设备上线事件，以及设备保持在线期间记录负责人通知的投递结果：可恢复或未能落库时按目标
/// 有界退避重试（同一条目最多计时重试 3 次，之后只等下一次上线事件），已结算时清除该目标的
/// 退避，均不依赖新的上线事件。
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
        let mut results = deps.delivery_results.subscribe();
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
                            // 上线事件立即恢复，此前的退避与已用尽标记不再适用：这次补发
                            // 若仍未送达，开启新的 3 次计时重试窗口，每次上线至多一个窗口。
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
                        Ok(result) => {
                            apply_delivery_result(&task_deps, &mut schedule, result, &task_cancel)
                                .await;
                        }
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

/// 记录负责人每次写入结束后的结果通知是恢复负责人唯一的结果来源：未结束或未能落库的
/// 投递按目标退避再试，已结算的内容清除对应目标的退避。
///
/// 通知不携带先后：较旧条目的结果可能在较新条目之后才到（例如仍在后台收尾，仓库写入也会
/// 覆盖其已取代状态）。因此只有目标尚无退避状态或通知属于正在跟踪的条目时直接记录；
/// 属于另一条目时按恢复轮次同一来源（[`newest_recoverable_entry`]）判断目标当前的最新内容，
/// 只有最新内容不是正在跟踪的条目时才把它作为新内容记录，较旧内容的迟到结果不重置次数或
/// 已用尽标记。
async fn apply_delivery_result(
    deps: &OfflineDeliveryRecoveryDeps,
    schedule: &mut RetrySchedule,
    result: DeliveryResult,
    cancel: &CancellationToken,
) {
    let target = result.target_device_id;
    match result.kind {
        DeliveryResultKind::Recoverable | DeliveryResultKind::NotPersisted => {
            let tracked = match schedule.tracked_entry(&target) {
                Some(tracked) if *tracked != result.entry_id => tracked.clone(),
                _ => {
                    schedule.record_recoverable(target, Some(result.entry_id), Instant::now());
                    return;
                }
            };
            match newest_recoverable_entry(deps, &target, cancel).await {
                Ok(Some(newest)) if newest != tracked => {
                    schedule.record_recoverable(target, Some(newest), Instant::now());
                }
                Ok(_) | Err(ScanStopped::Cancelled) => {}
                // 无法判断先后时按恢复轮次自身失败处理：照常排期，但不重置次数。
                Err(ScanStopped::ListingFailed) => {
                    schedule.record_recoverable(target, None, Instant::now());
                }
            }
        }
        DeliveryResultKind::Settled => {
            schedule.settle(&target, &result.entry_id);
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

/// 执行一轮恢复，并按结局决定该目标的退避：未补发的终态清除，自身失败再排期，
/// 已发出的补发（含已确认的）记下所等待的条目，由记录负责人随后通知的结果决定再排期或清除。
async fn recover_and_schedule(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    trigger: RecoveryTrigger,
    schedule: &mut RetrySchedule,
    cancel: &CancellationToken,
) {
    let round = recover_online_target(deps, target, trigger, cancel).await;
    match round.outcome.retry_after_round() {
        RetryAfterRound::Clear => schedule.clear(&target),
        RetryAfterRound::Again => schedule.record_recoverable(target, None, Instant::now()),
        RetryAfterRound::AwaitResult => {
            if let Some(entry) = round.awaiting {
                schedule.await_result(&target, entry);
            }
        }
    }
}

/// 一轮恢复的完整动作：触发原因由调用方给出，结局由 `recover_for_target` 返回，这里写唯一的一条记录。
async fn recover_online_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    trigger: RecoveryTrigger,
    cancel: &CancellationToken,
) -> RecoveryRound {
    let _gate = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            record_recovery(trigger, RecoveryOutcome::Cancelled);
            return RecoveryOutcome::Cancelled.into();
        }
        gate = deps.delivery_gate.lock() => gate,
    };
    let round = recover_for_target(deps, target, cancel).await;
    record_recovery(trigger, round.outcome);
    round
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
) -> RecoveryRound {
    if cancel.is_cancelled() {
        return RecoveryOutcome::Cancelled.into();
    }
    if !automatic_sync_enabled(deps.settings.as_ref()).await {
        return RecoveryOutcome::SyncDisabled.into();
    }

    let scope = match deps.member_scope.snapshot().await {
        Ok(scope) if scope.local_member_active => scope,
        Ok(_) => return RecoveryOutcome::MemberInactive.into(),
        Err(_) => return RecoveryOutcome::ScopeUnavailable.into(),
    };
    let target_is_current = scope.usable_peer_device_ids.contains(&target);
    if !target_is_current
        && scope
            .paused_peer_devices
            .iter()
            .any(|peer| peer.device_id == target)
    {
        return RecoveryOutcome::TargetPaused.into();
    }

    let (entry_id, record) = match newest_delivery_for_target(deps, &target, cancel).await {
        Ok(Some(found)) => found,
        Ok(None) => return RecoveryOutcome::NothingToRecover.into(),
        Err(ScanStopped::Cancelled) => return RecoveryOutcome::Cancelled.into(),
        Err(ScanStopped::ListingFailed) => return RecoveryOutcome::ListingFailed.into(),
    };
    if !target_is_current {
        supersede_delivery(deps, &entry_id, &target).await;
        return RecoveryOutcome::TargetNotCurrent.into();
    }
    // 旧内容失效是一项完整动作，必须完成后才处理停止，避免重启后补发旧内容。
    if !supersede_older_recoverable_entries(deps, &entry_id, &target).await {
        uc_warn!(
            entry_id = log_id(&entry_id),
            "clipboard delivery recovery: unable to replace older offline content"
        );
        return RecoveryOutcome::SupersedeFailed.into();
    }
    if cancel.is_cancelled() {
        return RecoveryOutcome::Cancelled.into();
    }
    if !matches!(
        record.status,
        EntryDeliveryStatus::Pending | EntryDeliveryStatus::Unreachable
    ) {
        return RecoveryOutcome::StatusChanged.into();
    }
    if !automatic_sync_enabled(deps.settings.as_ref()).await {
        return RecoveryOutcome::SyncDisabled.into();
    }
    if cancel.is_cancelled() {
        return RecoveryOutcome::Cancelled.into();
    }
    match deps
        .delivery
        .deliver_existing_local_entry(entry_id.clone(), vec![target])
        .await
    {
        Ok(report) => {
            uc_info!(
                entry_id = log_id(&entry_id),
                accepted = report.accepted,
                duplicate = report.duplicate,
                offline = report.offline,
                errored = report.errored,
                pending = report.pending,
                "clipboard delivery recovery dispatched"
            );
            let outcome = if report.accepted + report.duplicate > 0 {
                RecoveryOutcome::Delivered
            } else {
                RecoveryOutcome::Dispatched
            };
            // 结算（送达、终态）、可恢复结果、写入失败或后台收尾都由记录负责人另行通知；
            // 确认后最终写入失败时持久状态仍未结束，不能据确认提前清除退避。
            RecoveryRound {
                outcome,
                awaiting: Some(entry_id),
            }
        }
        Err(ResendEntryError::EntryNotResendable { reason, .. }) => {
            if matches!(reason, NotResendableReason::PayloadLost) {
                uc_info!(
                    entry_id = log_id(&entry_id),
                    reason = "payload_lost",
                    "clipboard delivery recovery stopped: entry payload is gone"
                );
                stop_automatic_recovery(deps, &entry_id, &target).await;
                RecoveryOutcome::PayloadLost.into()
            } else {
                RecoveryOutcome::NotResendable.into()
            }
        }
        Err(error @ (ResendEntryError::Storage(_) | ResendEntryError::Dispatch(_))) => {
            uc_warn!(
                error_kind = "delivery",
                reason = error.kind(),
                entry_id = log_id(&entry_id),
                "clipboard delivery recovery skipped entry"
            );
            RecoveryOutcome::DeliveryFailed.into()
        }
        Err(error) => {
            uc_debug!(
                error_kind = "delivery",
                reason = error.kind(),
                entry_id = log_id(&entry_id),
                "clipboard delivery recovery skipped entry"
            );
            RecoveryOutcome::DeliverySkipped.into()
        }
    }
}

/// 扫描为什么提前结束。
enum ScanStopped {
    Cancelled,
    ListingFailed,
}

/// 目标当前的最新内容：按新到旧扫描本机发出且跟踪投递的条目，第一条带有该目标投递记录的
/// 条目及其记录。恢复轮次只补发它，较旧内容随后被标记为已取代；结果通知的先后也只按它判断，
/// 不另建条目顺序。读取单条条目失败时跳过该条目，列举失败时中止扫描。
async fn newest_delivery_for_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: &DeviceId,
    cancel: &CancellationToken,
) -> Result<Option<(EntryId, EntryDeliveryRecord)>, ScanStopped> {
    let local_device = deps.device_identity.current_device_id();
    let mut offset = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(ScanStopped::Cancelled);
        }
        let entries = match deps.entries.list_entries(RECOVERY_PAGE_SIZE, offset).await {
            Ok(entries) => entries,
            Err(_) => {
                uc_warn!(
                    error_kind = "entry_listing",
                    "clipboard delivery recovery: scan stopped"
                );
                return Err(ScanStopped::ListingFailed);
            }
        };
        if entries.is_empty() {
            return Ok(None);
        }
        let count = entries.len();
        for entry in entries {
            if cancel.is_cancelled() {
                return Err(ScanStopped::Cancelled);
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
                return Err(ScanStopped::Cancelled);
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
                return Err(ScanStopped::Cancelled);
            }
            if let Some(record) = records
                .into_iter()
                .find(|record| record.target_device_id == *target)
            {
                return Ok(Some((entry.entry_id, record)));
            }
        }
        if count < RECOVERY_PAGE_SIZE {
            return Ok(None);
        }
        offset += count;
    }
}

/// 目标当前最新内容仍未结束（`Pending`/`Unreachable`）时返回该条目；最新内容已结算或没有
/// 记录时返回 `None`。
async fn newest_recoverable_entry(
    deps: &OfflineDeliveryRecoveryDeps,
    target: &DeviceId,
    cancel: &CancellationToken,
) -> Result<Option<EntryId>, ScanStopped> {
    Ok(newest_delivery_for_target(deps, target, cancel)
        .await?
        .filter(|(_, record)| is_recovery_eligible(record, target))
        .map(|(entry_id, _)| entry_id))
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
