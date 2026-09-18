use std::sync::Arc;

use async_trait::async_trait;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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

#[cfg(test)]
mod tests;

const RECOVERY_PAGE_SIZE: usize = 256;

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
                            recover_online_target(&task_deps, event.device_id, &task_cancel).await;
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            warn!(missed, "clipboard delivery recovery: presence events lagged");
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
                warn!(
                    entry_id = %entry_id,
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
            warn!(
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
            recover_online_target(deps, peer.device_id, cancel).await;
        }
    }
}

async fn recover_online_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    cancel: &CancellationToken,
) {
    let _gate = tokio::select! {
        biased;
        _ = cancel.cancelled() => return,
        gate = deps.delivery_gate.lock() => gate,
    };
    recover_for_target(deps, target, cancel).await;
}

async fn recover_for_target(
    deps: &OfflineDeliveryRecoveryDeps,
    target: DeviceId,
    cancel: &CancellationToken,
) {
    if cancel.is_cancelled() || !automatic_sync_enabled(deps.settings.as_ref()).await {
        return;
    }

    let scope = match deps.member_scope.snapshot().await {
        Ok(scope) if scope.local_member_active => scope,
        Ok(_) | Err(_) => return,
    };
    let target_is_current = scope.usable_peer_device_ids.contains(&target);
    if !target_is_current
        && scope
            .paused_peer_devices
            .iter()
            .any(|peer| peer.device_id == target)
    {
        return;
    }

    let local_device = deps.device_identity.current_device_id();
    let mut offset = 0;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let entries = match deps.entries.list_entries(RECOVERY_PAGE_SIZE, offset).await {
            Ok(entries) => entries,
            Err(_) => {
                warn!(
                    error_kind = "entry_listing",
                    "clipboard delivery recovery: scan stopped"
                );
                return;
            }
        };
        if entries.is_empty() {
            return;
        }
        let count = entries.len();
        for entry in entries {
            if cancel.is_cancelled() {
                return;
            }
            if !entry.delivery_tracked {
                continue;
            }
            let source = match deps.events.get_source_device(&entry.event_id).await {
                Ok(source) => source,
                Err(_) => {
                    warn!(
                        error_kind = "entry_source",
                        "clipboard delivery recovery: entry skipped"
                    );
                    continue;
                }
            };
            if cancel.is_cancelled() {
                return;
            }
            if source.as_ref() != Some(&local_device) {
                continue;
            }
            let records = match deps.deliveries.list_by_entry(&entry.entry_id).await {
                Ok(records) => records,
                Err(_) => {
                    warn!(
                        error_kind = "delivery_lookup",
                        "clipboard delivery recovery: entry skipped"
                    );
                    continue;
                }
            };
            if cancel.is_cancelled() {
                return;
            }
            let Some(record) = records
                .iter()
                .find(|record| record.target_device_id == target)
            else {
                continue;
            };
            if !target_is_current {
                supersede_delivery(deps, &entry.entry_id, &target).await;
                return;
            }
            // 旧内容失效是一项完整动作，必须完成后才处理停止，避免重启后补发旧内容。
            if !supersede_older_recoverable_entries(deps, &entry.entry_id, &target).await {
                warn!(
                    entry_id = %entry.entry_id,
                    "clipboard delivery recovery: unable to replace older offline content"
                );
                return;
            }
            if cancel.is_cancelled()
                || !matches!(
                    record.status,
                    EntryDeliveryStatus::Pending | EntryDeliveryStatus::Unreachable
                )
            {
                return;
            }
            if !automatic_sync_enabled(deps.settings.as_ref()).await || cancel.is_cancelled() {
                return;
            }
            match deps
                .delivery
                .deliver_existing_local_entry(entry.entry_id.clone(), vec![target.clone()])
                .await
            {
                Ok(report) => info!(
                    entry_id = %entry.entry_id,
                    accepted = report.accepted,
                    duplicate = report.duplicate,
                    offline = report.offline,
                    errored = report.errored,
                    pending = report.pending,
                    "clipboard delivery recovery dispatched"
                ),
                Err(ResendEntryError::EntryNotResendable { reason, .. }) => {
                    if matches!(reason, NotResendableReason::PayloadLost) {
                        stop_automatic_recovery(deps, &entry.entry_id, &target).await;
                    }
                }
                Err(_) => {
                    debug!(error_kind = "delivery", entry_id = %entry.entry_id, "clipboard delivery recovery skipped entry");
                }
            }
            return;
        }
        if count < RECOVERY_PAGE_SIZE {
            return;
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
        warn!(
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
        warn!(
            error_kind = "delivery_record",
            "clipboard delivery recovery: failed to stop ineligible target recovery"
        );
    }
}
