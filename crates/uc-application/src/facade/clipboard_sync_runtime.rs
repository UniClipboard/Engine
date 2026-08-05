//! Owns automatic clipboard delivery after local capture and peer recovery.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use uc_core::clipboard::{DeliveryFailureReason, EntryDeliveryRecord, EntryDeliveryStatus};
use uc_core::ids::DeviceId;
use uc_core::ports::clipboard::{ClipboardEventRepositoryPort, ListClipboardEntriesPort};
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, EntryDeliveryRepositoryPort, PeerAddressRepositoryPort,
    PresencePort, ReachabilityState, SettingsPort,
};

use super::{
    ClipboardInboundRuntime, ClipboardOutboundError, ClipboardOutboundFacade,
    ClipboardOutboundInput, ClipboardOutboundOutcome, ResendEntryCommand, ResendEntryError,
    ResendReport,
};
const RECOVERY_PAGE_SIZE: usize = 256;

/// The complete automatic outbound lifecycle. Callers submit local captures
/// and manual resends here; peer recovery stays internal to this runtime.
pub struct ClipboardSyncRuntime {
    outbound: Arc<ClipboardOutboundFacade>,
    settings: Arc<dyn SettingsPort>,
    inbound: tokio::sync::Mutex<Option<ClipboardInboundRuntime>>,
    recovery: OfflineDeliveryRecovery,
}

pub struct ClipboardSyncRuntimeDeps {
    pub outbound: Arc<ClipboardOutboundFacade>,
    pub settings: Arc<dyn SettingsPort>,
    pub inbound: ClipboardInboundRuntime,
    pub presence: Arc<dyn PresencePort>,
    pub known_peers: Arc<dyn PeerAddressRepositoryPort>,
    pub entries: Arc<dyn ListClipboardEntriesPort>,
    pub events: Arc<dyn ClipboardEventRepositoryPort>,
    pub deliveries: Arc<dyn EntryDeliveryRepositoryPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    pub clock: Arc<dyn ClockPort>,
}

impl ClipboardSyncRuntime {
    pub fn start(deps: ClipboardSyncRuntimeDeps) -> Self {
        let recovery = OfflineDeliveryRecovery::start(OfflineDeliveryRecoveryDeps {
            presence: deps.presence,
            known_peers: deps.known_peers,
            settings: Arc::clone(&deps.settings),
            entries: deps.entries,
            events: deps.events,
            deliveries: deps.deliveries,
            device_identity: deps.device_identity,
            clock: deps.clock,
            delivery: Arc::clone(&deps.outbound) as Arc<dyn RecoveryDeliveryPort>,
        });
        Self {
            outbound: deps.outbound,
            settings: deps.settings,
            inbound: tokio::sync::Mutex::new(Some(deps.inbound)),
            recovery,
        }
    }

    /// Sends a newly captured local clipboard entry only when automatic sync
    /// is enabled. A disabled capture creates no delivery attempt, so it can
    /// never become a later recovery candidate.
    pub async fn dispatch_local_capture(
        &self,
        input: ClipboardOutboundInput,
    ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
        if !auto_sync_enabled(self.settings.as_ref()).await {
            return Ok(ClipboardOutboundOutcome::Skipped {
                reason: "auto_sync_disabled".to_string(),
            });
        }
        self.outbound.dispatch_capture(input).await
    }

    /// Manual resend remains explicit and is intentionally independent of the
    /// automatic-sync toggle.
    pub async fn resend_entry(
        &self,
        command: ResendEntryCommand,
    ) -> Result<ResendReport, ResendEntryError> {
        self.outbound.resend_entry(command).await
    }

    pub async fn shutdown(&self) {
        self.recovery.shutdown().await;
        if let Some(inbound) = self.inbound.lock().await.take() {
            if inbound.shutdown().await.is_err() {
                warn!(
                    error_kind = "inbound_shutdown",
                    "clipboard sync: inbound runtime stopped unexpectedly"
                );
            }
        }
    }
}

async fn auto_sync_enabled(settings: &dyn SettingsPort) -> bool {
    match settings.load().await {
        Ok(settings) => settings.sync.auto_sync,
        Err(_) => {
            warn!(
                error_kind = "settings_load",
                "clipboard sync: automatic delivery skipped"
            );
            false
        }
    }
}

fn is_recovery_eligible(record: &EntryDeliveryRecord, target: &DeviceId) -> bool {
    record.target_device_id == *target && matches!(record.status, EntryDeliveryStatus::Unreachable)
}

#[async_trait]
trait RecoveryDeliveryPort: Send + Sync {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: uc_core::ids::EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError>;
}

#[async_trait]
impl RecoveryDeliveryPort for ClipboardOutboundFacade {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: uc_core::ids::EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        ClipboardOutboundFacade::deliver_existing_local_entry(self, entry_id, targets).await
    }
}

struct OfflineDeliveryRecoveryDeps {
    presence: Arc<dyn PresencePort>,
    known_peers: Arc<dyn PeerAddressRepositoryPort>,
    settings: Arc<dyn SettingsPort>,
    entries: Arc<dyn ListClipboardEntriesPort>,
    events: Arc<dyn ClipboardEventRepositoryPort>,
    deliveries: Arc<dyn EntryDeliveryRepositoryPort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    clock: Arc<dyn ClockPort>,
    delivery: Arc<dyn RecoveryDeliveryPort>,
}

/// Watches reachability transitions and restores only the delivery facts that
/// were previously recorded as temporarily unreachable.
struct OfflineDeliveryRecovery {
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl OfflineDeliveryRecovery {
    fn start(deps: OfflineDeliveryRecoveryDeps) -> Self {
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut events = deps.presence.subscribe();
            recover_currently_online(&deps).await;
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => return,
                    event = events.recv() => match event {
                        Ok(event) if event.state == ReachabilityState::Online => {
                            recover_for_target(&deps, event.device_id).await;
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
        }
    }

    async fn shutdown(&self) {
        self.cancel.cancel();
        if let Some(task) = self.task.lock().await.take() {
            if task.await.is_err() {
                warn!(
                    error_kind = "task",
                    "clipboard delivery recovery task stopped unexpectedly"
                );
            }
        }
    }
}

async fn recover_currently_online(deps: &OfflineDeliveryRecoveryDeps) {
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
        if deps.presence.current_state(&peer.device_id).await == ReachabilityState::Online {
            recover_for_target(deps, peer.device_id).await;
        }
    }
}

async fn recover_for_target(deps: &OfflineDeliveryRecoveryDeps, target: DeviceId) {
    if !auto_sync_enabled(deps.settings.as_ref()).await {
        return;
    }

    let local_device = deps.device_identity.current_device_id();
    let mut offset = 0;
    loop {
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
            if !records
                .iter()
                .any(|record| is_recovery_eligible(record, &target))
            {
                continue;
            }
            if !auto_sync_enabled(deps.settings.as_ref()).await {
                return;
            }
            match deps
                .delivery
                .deliver_existing_local_entry(entry.entry_id.clone(), vec![target.clone()])
                .await
            {
                Ok(report) => info!(
                    entry_id = %entry.entry_id,
                    target = %target,
                    accepted = report.accepted,
                    duplicate = report.duplicate,
                    offline = report.offline,
                    errored = report.errored,
                    pending = report.pending,
                    "clipboard delivery recovery dispatched"
                ),
                Err(ResendEntryError::EntryNotResendable { reason, .. }) => {
                    if matches!(reason, super::NotResendableReason::PayloadLost) {
                        stop_automatic_recovery(deps, &entry.entry_id, &target).await;
                    }
                }
                Err(_) => {
                    debug!(error_kind = "delivery", entry_id = %entry.entry_id, target = %target, "clipboard delivery recovery skipped entry");
                }
            }
        }
        if count < RECOVERY_PAGE_SIZE {
            return;
        }
        offset += count;
    }
}

async fn stop_automatic_recovery(
    deps: &OfflineDeliveryRecoveryDeps,
    entry_id: &uc_core::ids::EntryId,
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Mutex;

    use uc_core::clipboard::{ClipboardEntry, ClipboardRepositoryError};
    use uc_core::ids::{EntryId, EventId};
    use uc_core::ports::presence::{PresenceError, PresenceEvent};
    use uc_core::settings::model::Settings;

    struct FixedSettings {
        auto_sync: bool,
    }

    #[async_trait]
    impl SettingsPort for FixedSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            let mut settings = Settings::default();
            settings.sync.auto_sync = self.auto_sync;
            Ok(settings)
        }

        async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct Entries(Vec<ClipboardEntry>);

    #[async_trait]
    impl ListClipboardEntriesPort for Entries {
        async fn list_entries(
            &self,
            limit: usize,
            offset: usize,
        ) -> Result<Vec<ClipboardEntry>, ClipboardRepositoryError> {
            Ok(self.0.iter().skip(offset).take(limit).cloned().collect())
        }
    }

    struct Sources {
        sources: HashMap<EventId, DeviceId>,
    }

    #[async_trait]
    impl ClipboardEventRepositoryPort for Sources {
        async fn get_representation(
            &self,
            _id: &EventId,
            _representation_id: &str,
        ) -> anyhow::Result<uc_core::ObservedClipboardRepresentation> {
            Err(anyhow::anyhow!("not used by delivery recovery"))
        }

        async fn get_source_device(&self, event_id: &EventId) -> anyhow::Result<Option<DeviceId>> {
            Ok(self.sources.get(event_id).cloned())
        }
    }

    struct Deliveries {
        records: Mutex<HashMap<EntryId, Vec<EntryDeliveryRecord>>>,
    }

    #[async_trait]
    impl EntryDeliveryRepositoryPort for Deliveries {
        async fn record_attempt(
            &self,
            record: &EntryDeliveryRecord,
        ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
            self.records
                .lock()
                .unwrap()
                .insert(record.entry_id.clone(), vec![record.clone()]);
            Ok(())
        }

        async fn list_by_entry(
            &self,
            entry_id: &EntryId,
        ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .get(entry_id)
                .cloned()
                .unwrap_or_default())
        }
    }

    struct LocalDevice(DeviceId);

    impl DeviceIdentityPort for LocalDevice {
        fn current_device_id(&self) -> DeviceId {
            self.0.clone()
        }
    }

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            42
        }
    }

    struct RecordingDispatch {
        commands: Mutex<Vec<(EntryId, Vec<DeviceId>)>>,
        result: DispatchResult,
    }

    enum DispatchResult {
        Delivered,
        PayloadLost,
    }

    #[async_trait]
    impl RecoveryDeliveryPort for RecordingDispatch {
        async fn deliver_existing_local_entry(
            &self,
            entry_id: EntryId,
            targets: Vec<DeviceId>,
        ) -> Result<ResendReport, ResendEntryError> {
            self.commands.lock().unwrap().push((entry_id, targets));
            match self.result {
                DispatchResult::Delivered => Ok(ResendReport {
                    accepted: 1,
                    duplicate: 0,
                    offline: 0,
                    errored: 0,
                    pending: 0,
                }),
                DispatchResult::PayloadLost => Err(ResendEntryError::EntryNotResendable {
                    entry_id: EntryId::from("offline-entry"),
                    reason: crate::facade::NotResendableReason::PayloadLost,
                }),
            }
        }
    }

    struct IdlePresence {
        tx: tokio::sync::broadcast::Sender<PresenceEvent>,
    }

    #[async_trait]
    impl PresencePort for IdlePresence {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            Ok(ReachabilityState::Unknown)
        }

        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            ReachabilityState::Unknown
        }

        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PresenceEvent> {
            self.tx.subscribe()
        }
    }

    struct NoPeers;

    #[async_trait]
    impl PeerAddressRepositoryPort for NoPeers {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(None)
        }

        async fn upsert(
            &self,
            _record: &uc_core::ports::PeerAddressRecord,
        ) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(Vec::new())
        }

        async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }
    }

    fn entry(id: &str, event_id: &str) -> ClipboardEntry {
        ClipboardEntry::new(EntryId::from(id), EventId::from(event_id), 1, 1)
    }

    fn recovery_deps(
        auto_sync: bool,
        entries: Vec<ClipboardEntry>,
        sources: HashMap<EventId, DeviceId>,
        deliveries: Arc<Deliveries>,
        delivery: Arc<RecordingDispatch>,
    ) -> OfflineDeliveryRecoveryDeps {
        let (tx, _) = tokio::sync::broadcast::channel(1);
        OfflineDeliveryRecoveryDeps {
            presence: Arc::new(IdlePresence { tx }),
            known_peers: Arc::new(NoPeers),
            settings: Arc::new(FixedSettings { auto_sync }),
            entries: Arc::new(Entries(entries)),
            events: Arc::new(Sources { sources }),
            deliveries,
            device_identity: Arc::new(LocalDevice(DeviceId::new("local"))),
            clock: Arc::new(FixedClock),
            delivery,
        }
    }

    #[tokio::test]
    async fn recovery_only_dispatches_the_recovered_devices_unreachable_local_entry() {
        let offline_entry = entry("offline-entry", "local-event");
        let delivered_entry = entry("delivered-entry", "local-event-2");
        let remote_entry = entry("remote-entry", "remote-event");
        let target = DeviceId::new("recovered");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::from([
                (
                    offline_entry.entry_id.clone(),
                    vec![EntryDeliveryRecord {
                        entry_id: offline_entry.entry_id.clone(),
                        target_device_id: target.clone(),
                        status: EntryDeliveryStatus::Unreachable,
                        reason_detail: None,
                        updated_at_ms: 1,
                    }],
                ),
                (
                    delivered_entry.entry_id.clone(),
                    vec![EntryDeliveryRecord {
                        entry_id: delivered_entry.entry_id.clone(),
                        target_device_id: target.clone(),
                        status: EntryDeliveryStatus::Delivered,
                        reason_detail: None,
                        updated_at_ms: 1,
                    }],
                ),
                (
                    remote_entry.entry_id.clone(),
                    vec![EntryDeliveryRecord {
                        entry_id: remote_entry.entry_id.clone(),
                        target_device_id: target.clone(),
                        status: EntryDeliveryStatus::Unreachable,
                        reason_detail: None,
                        updated_at_ms: 1,
                    }],
                ),
            ])),
        });
        let delivery = Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::Delivered,
        });
        let deps = recovery_deps(
            true,
            vec![offline_entry.clone(), delivered_entry, remote_entry],
            HashMap::from([
                (offline_entry.event_id.clone(), DeviceId::new("local")),
                (EventId::from("local-event-2"), DeviceId::new("local")),
                (EventId::from("remote-event"), DeviceId::new("remote")),
            ]),
            deliveries,
            Arc::clone(&delivery),
        );

        recover_for_target(&deps, target.clone()).await;

        let commands = delivery.commands.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0], (offline_entry.entry_id, vec![target]));
    }

    #[tokio::test]
    async fn disabled_auto_sync_never_dispatches_a_saved_offline_delivery() {
        let pending_entry = entry("offline-entry", "local-event");
        let target = DeviceId::new("recovered");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::from([(
                pending_entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: pending_entry.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Unreachable,
                    reason_detail: None,
                    updated_at_ms: 1,
                }],
            )])),
        });
        let delivery = Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::Delivered,
        });
        let deps = recovery_deps(
            false,
            vec![pending_entry.clone()],
            HashMap::from([(pending_entry.event_id.clone(), DeviceId::new("local"))]),
            deliveries,
            Arc::clone(&delivery),
        );

        recover_for_target(&deps, target).await;

        assert!(delivery.commands.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn payload_lost_stops_future_automatic_recovery_for_that_entry() {
        let pending_entry = entry("offline-entry", "local-event");
        let target = DeviceId::new("recovered");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::from([(
                pending_entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: pending_entry.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Unreachable,
                    reason_detail: None,
                    updated_at_ms: 1,
                }],
            )])),
        });
        let delivery = Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::PayloadLost,
        });
        let deps = recovery_deps(
            true,
            vec![pending_entry.clone()],
            HashMap::from([(pending_entry.event_id.clone(), DeviceId::new("local"))]),
            Arc::clone(&deliveries),
            delivery,
        );

        recover_for_target(&deps, target.clone()).await;

        let stored = deliveries
            .list_by_entry(&pending_entry.entry_id)
            .await
            .unwrap();
        assert!(matches!(
            stored[0].status,
            EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::Internal
            }
        ));
        assert_eq!(stored[0].target_device_id, target);
    }
}
