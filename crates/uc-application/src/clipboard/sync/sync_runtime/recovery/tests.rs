use super::super::ClipboardSyncRuntime;
use super::*;
use crate::clipboard::inbound::ClipboardInboundRuntime;
use crate::clipboard::outbound::{
    ClipboardOutboundError, ClipboardOutboundInput, ClipboardOutboundOutcome, ClipboardOutboundPort,
};
use std::sync::atomic::{AtomicBool, Ordering};

use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Write};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Notify;

use crate::deps::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    PausedSpaceMember, SpaceMemberPauseReason,
};
use uc_core::clipboard::{ClipboardEntry, ClipboardRepositoryError};
use uc_core::ids::{EntryId, EventId};
use uc_core::ports::peer_reachability::{PeerReachabilityChanged, PeerReachabilityError};
use uc_core::settings::model::Settings;
use uc_core::{ClipboardChangeOrigin, SystemClipboardSnapshot};

struct NeverDispatch;

#[derive(Clone, Default)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl CapturedWriter {
    fn output(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedWriter {
    type Writer = CapturedWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

struct SkippingOutbound;

#[async_trait]
impl ClipboardOutboundPort for SkippingOutbound {
    async fn dispatch_capture(
        &self,
        _: ClipboardOutboundInput,
        _: Option<Vec<DeviceId>>,
    ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
        Ok(ClipboardOutboundOutcome::Skipped {
            reason: "test".to_owned(),
        })
    }
}

#[async_trait]
impl ClipboardOutboundPort for NeverDispatch {
    async fn dispatch_capture(
        &self,
        _: ClipboardOutboundInput,
        _: Option<Vec<DeviceId>>,
    ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
        panic!("stopped runtime dispatched clipboard content");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn local_dispatch_records_real_delivery_gate_wait() {
    let deps = recovery_deps(
        true,
        Vec::new(),
        HashMap::new(),
        Arc::new(Deliveries {
            records: Mutex::new(HashMap::new()),
        }),
        Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::Delivered,
        }),
    );
    let gate = Arc::clone(&deps.delivery_gate);
    let held = Arc::clone(&gate).lock_owned().await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        drop(held);
    });
    let runtime = ClipboardSyncRuntime {
        outbound: Arc::new(SkippingOutbound),
        settings: Arc::clone(&deps.settings),
        inbound: tokio::sync::Mutex::new(None),
        delivery_gate: gate,
        recovery: OfflineDeliveryRecovery {
            cancel: CancellationToken::new(),
            task: tokio::sync::Mutex::new(None),
            deps: Arc::new(deps),
        },
        stopping: AtomicBool::new(false),
        shutdown_result: tokio::sync::Mutex::new(None),
    };
    let logs = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    runtime
        .dispatch_local_capture_to_targets(
            ClipboardOutboundInput {
                entry_id: "entry".to_owned(),
                snapshot: SystemClipboardSnapshot {
                    representations: Vec::new(),
                    ts_ms: 0,
                    file_content_digests: Vec::new(),
                    file_set_v1_component: None,
                },
                origin: ClipboardChangeOrigin::LocalCapture,
            },
            None,
        )
        .await
        .unwrap();
    release.await.unwrap();

    let output = logs.output();
    assert!(output.contains("clipboard_delivery_gate_wait"));
    assert!(output.contains("clipboard_sync_settings_load"));
}

#[tokio::test]
async fn shutdown_survives_waiter_cancellation_and_retains_both_worker_failures() {
    let deps = recovery_deps(
        true,
        Vec::new(),
        HashMap::new(),
        Arc::new(Deliveries {
            records: Mutex::new(HashMap::new()),
        }),
        Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::Delivered,
        }),
    );
    let gate = Arc::clone(&deps.delivery_gate);
    let dispatch = gate.lock().await;
    let runtime = Arc::new(ClipboardSyncRuntime {
        outbound: Arc::new(NeverDispatch),
        settings: Arc::clone(&deps.settings),
        inbound: tokio::sync::Mutex::new(Some(ClipboardInboundRuntime::from_task(tokio::spawn(
            async { panic!("PRIVATE_INBOUND_FAILURE") },
        )))),
        delivery_gate: Arc::clone(&gate),
        recovery: OfflineDeliveryRecovery {
            cancel: CancellationToken::new(),
            task: tokio::sync::Mutex::new(Some(tokio::spawn(async {
                panic!("PRIVATE_RECOVERY_FAILURE")
            }))),
            deps: Arc::new(deps),
        },
        stopping: AtomicBool::new(false),
        shutdown_result: tokio::sync::Mutex::new(None),
    });
    let waiter = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move { runtime.shutdown().await })
    };
    runtime.recovery.cancel.cancelled().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let confirmation = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move { runtime.shutdown().await })
    };
    tokio::task::yield_now().await;
    assert!(!confirmation.is_finished());
    drop(dispatch);
    let error = confirmation.await.unwrap().unwrap_err();
    assert!(error
        .primary
        .downcast_ref::<JoinError>()
        .unwrap()
        .is_panic());
    assert_eq!(error.additional.len(), 1);
    let inbound = error.additional[0]
        .downcast_ref::<crate::clipboard::inbound::ClipboardInboundRuntimeError>()
        .unwrap();
    assert!(inbound.source().unwrap().is::<JoinError>());
    assert!(!format!("{error:?} {error}").contains("PRIVATE"));
    assert!(Arc::ptr_eq(&error, &runtime.shutdown().await.unwrap_err()));
    let result = runtime
        .dispatch_local_capture_to_targets(
            ClipboardOutboundInput {
                entry_id: "entry".to_owned(),
                snapshot: SystemClipboardSnapshot {
                    representations: Vec::new(),
                    ts_ms: 0,
                    file_content_digests: Vec::new(),
                    file_set_v1_component: None,
                },
                origin: ClipboardChangeOrigin::LocalCapture,
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(result, ClipboardOutboundOutcome::Skipped { reason } if reason == "runtime_stopped")
    );
}

struct FixedSettings {
    sync_enabled: bool,
    auto_sync_enabled: bool,
}

#[async_trait]
impl SettingsPort for FixedSettings {
    async fn load(&self) -> anyhow::Result<Settings> {
        let mut settings = Settings::default();
        settings.sync.sync_enabled = self.sync_enabled;
        settings.sync.auto_sync_enabled = self.auto_sync_enabled;
        Ok(settings)
    }

    async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
        Ok(())
    }
}

struct Entries(Vec<ClipboardEntry>);

struct BlockingEntries {
    entries: Vec<ClipboardEntry>,
    block_on: usize,
    calls: AtomicUsize,
    entered: Arc<Notify>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    completed: Arc<AtomicBool>,
}

#[async_trait]
impl ListClipboardEntriesPort for BlockingEntries {
    async fn list_entries(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ClipboardEntry>, ClipboardRepositoryError> {
        if self.calls.fetch_add(1, Ordering::AcqRel) + 1 == self.block_on {
            let release = self.release.lock().unwrap().take().unwrap();
            let entered = Arc::clone(&self.entered);
            let completed = Arc::clone(&self.completed);
            tokio::task::spawn_blocking(move || {
                entered.notify_one();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
                completed.store(true, Ordering::Release);
            })
            .await
            .unwrap();
        }
        Ok(self
            .entries
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }
}

#[tokio::test]
async fn recovery_stop_finishes_storage_and_started_replacement_without_dispatching() {
    for block_on in [1, 2] {
        let newest = entry("newest", "newest-event");
        let older = entry("older", "older-event");
        let target = DeviceId::new("target");
        let records = [newest.clone(), older.clone()]
            .into_iter()
            .map(|entry| {
                (
                    entry.entry_id.clone(),
                    vec![EntryDeliveryRecord {
                        entry_id: entry.entry_id,
                        target_device_id: target,
                        status: EntryDeliveryStatus::Unreachable,
                        reason_detail: None,
                        updated_at_ms: 1,
                    }],
                )
            })
            .collect();
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(records),
        });
        let delivery = Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::Delivered,
        });
        let mut deps = recovery_deps(
            true,
            Vec::new(),
            HashMap::from([
                (newest.event_id.clone(), DeviceId::new("local")),
                (older.event_id.clone(), DeviceId::new("local")),
            ]),
            Arc::clone(&deliveries),
            Arc::clone(&delivery),
        );
        let (release, released) = std::sync::mpsc::channel();
        let entries = Arc::new(BlockingEntries {
            entries: vec![newest, older.clone()],
            block_on,
            calls: AtomicUsize::new(0),
            entered: Arc::new(Notify::new()),
            release: Mutex::new(Some(released)),
            completed: Arc::new(AtomicBool::new(false)),
        });
        deps.entries = entries.clone();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            recover_online_target(&deps, target, &task_cancel).await;
        });
        tokio::time::timeout(Duration::from_secs(2), entries.entered.notified())
            .await
            .unwrap();
        cancel.cancel();
        tokio::task::yield_now().await;
        let finished_before_storage = task.is_finished();
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!finished_before_storage);
        assert!(entries.completed.load(Ordering::Acquire));
        assert_eq!(entries.calls.load(Ordering::Acquire), block_on);
        assert!(delivery.commands.lock().unwrap().is_empty());
        let records = deliveries.list_by_entry(&older.entry_id).await.unwrap();
        if block_on == 2 {
            assert!(matches!(records[0].status, EntryDeliveryStatus::Superseded));
        } else {
            assert!(matches!(
                records[0].status,
                EntryDeliveryStatus::Unreachable
            ));
        }
    }
}

#[tokio::test]
async fn recovery_stop_leaves_the_delivery_queue_without_waiting_for_its_owner() {
    let deps = recovery_deps(
        true,
        Vec::new(),
        HashMap::new(),
        Arc::new(Deliveries {
            records: Mutex::new(HashMap::new()),
        }),
        Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: DispatchResult::Delivered,
        }),
    );
    let gate = Arc::clone(&deps.delivery_gate);
    let held = gate.lock().await;
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        recover_online_target(&deps, DeviceId::new("target"), &task_cancel).await;
    });
    tokio::task::yield_now().await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task).await;
    drop(held);
    result.unwrap().unwrap();
}

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

struct IdlePeerReachability {
    tx: tokio::sync::broadcast::Sender<PeerReachabilityChanged>,
}

#[async_trait]
impl PeerReachabilityPort for IdlePeerReachability {
    async fn ensure_reachable(
        &self,
        _device: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        Ok(ReachabilityState::Unknown)
    }

    async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
        ReachabilityState::Unknown
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PeerReachabilityChanged> {
        self.tx.subscribe()
    }
}

struct NoPeers;

struct FixedMemberScope {
    usable: Vec<DeviceId>,
    paused: Vec<PausedSpaceMember>,
}

#[async_trait]
impl CurrentSpaceMemberScopePort for FixedMemberScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: self.usable.clone(),
            paused_peer_devices: self.paused.clone(),
        })
    }
}

#[async_trait]
impl PeerAddressRepositoryPort for NoPeers {
    async fn get(
        &self,
        _device: &DeviceId,
    ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
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
    ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
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
    auto_sync_enabled: bool,
    entries: Vec<ClipboardEntry>,
    sources: HashMap<EventId, DeviceId>,
    deliveries: Arc<Deliveries>,
    delivery: Arc<RecordingDispatch>,
) -> OfflineDeliveryRecoveryDeps {
    let (tx, _) = tokio::sync::broadcast::channel(1);
    OfflineDeliveryRecoveryDeps {
        peer_reachability: Arc::new(IdlePeerReachability { tx }),
        known_peers: Arc::new(NoPeers),
        member_scope: Arc::new(FixedMemberScope {
            usable: vec![DeviceId::new("target"), DeviceId::new("recovered")],
            paused: Vec::new(),
        }),
        settings: Arc::new(FixedSettings {
            sync_enabled: true,
            auto_sync_enabled,
        }),
        entries: Arc::new(Entries(entries)),
        events: Arc::new(Sources { sources }),
        deliveries,
        device_identity: Arc::new(LocalDevice(DeviceId::new("local"))),
        clock: Arc::new(FixedClock),
        delivery,
        delivery_gate: Arc::new(tokio::sync::Mutex::new(())),
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
    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;

    let commands = delivery.commands.lock().unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0], (offline_entry.entry_id, vec![target]));
}

#[tokio::test]
async fn recovery_dispatches_an_attempt_left_pending_by_process_exit() {
    let pending_entry = entry("pending-entry", "local-event");
    let target = DeviceId::new("recovered");
    let deliveries = Arc::new(Deliveries {
        records: Mutex::new(HashMap::from([(
            pending_entry.entry_id.clone(),
            vec![EntryDeliveryRecord {
                entry_id: pending_entry.entry_id.clone(),
                target_device_id: target,
                status: EntryDeliveryStatus::Pending,
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
        true,
        vec![pending_entry.clone()],
        HashMap::from([(pending_entry.event_id.clone(), DeviceId::new("local"))]),
        deliveries,
        Arc::clone(&delivery),
    );

    recover_for_target(&deps, target, &CancellationToken::new()).await;

    assert_eq!(
        delivery.commands.lock().unwrap().as_slice(),
        &[(pending_entry.entry_id, vec![target])]
    );
}

#[tokio::test]
async fn deleted_and_terminal_entries_never_resume_automatic_delivery() {
    let deleted = entry("deleted", "deleted-event");
    let cancelled = entry("cancelled", "cancelled-event");
    let failed = entry("failed", "failed-event");
    let target = DeviceId::new("recovered");
    let deliveries = Arc::new(Deliveries {
        records: Mutex::new(HashMap::from([
            (
                deleted.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: deleted.entry_id,
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Pending,
                    reason_detail: None,
                    updated_at_ms: 1,
                }],
            ),
            (
                cancelled.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: cancelled.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Superseded,
                    reason_detail: None,
                    updated_at_ms: 2,
                }],
            ),
            (
                failed.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: failed.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Failed {
                        reason: DeliveryFailureReason::Internal,
                    },
                    reason_detail: None,
                    updated_at_ms: 3,
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
        vec![cancelled.clone(), failed.clone()],
        HashMap::from([
            (cancelled.event_id, DeviceId::new("local")),
            (failed.event_id, DeviceId::new("local")),
        ]),
        deliveries,
        Arc::clone(&delivery),
    );

    recover_for_target(&deps, target, &CancellationToken::new()).await;

    assert!(delivery.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn target_that_lost_delivery_eligibility_becomes_terminal() {
    let pending = entry("pending", "pending-event");
    let target = DeviceId::new("revoked");
    let deliveries = Arc::new(Deliveries {
        records: Mutex::new(HashMap::from([(
            pending.entry_id.clone(),
            vec![EntryDeliveryRecord {
                entry_id: pending.entry_id.clone(),
                target_device_id: target.clone(),
                status: EntryDeliveryStatus::Pending,
                reason_detail: None,
                updated_at_ms: 1,
            }],
        )])),
    });
    let delivery = Arc::new(RecordingDispatch {
        commands: Mutex::new(Vec::new()),
        result: DispatchResult::Delivered,
    });
    let mut deps = recovery_deps(
        true,
        vec![pending.clone()],
        HashMap::from([(pending.event_id.clone(), DeviceId::new("local"))]),
        Arc::clone(&deliveries),
        Arc::clone(&delivery),
    );
    deps.member_scope = Arc::new(FixedMemberScope {
        usable: Vec::new(),
        paused: Vec::new(),
    });

    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;
    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;

    assert!(delivery.commands.lock().unwrap().is_empty());
    let stored = deliveries.list_by_entry(&pending.entry_id).await.unwrap();
    assert!(matches!(
        stored.as_slice(),
        [record]
            if record.target_device_id == target
                && record.status == EntryDeliveryStatus::Superseded
    ));
}

#[tokio::test]
async fn temporarily_paused_target_keeps_its_pending_delivery() {
    let pending = entry("pending", "pending-event");
    let target = DeviceId::new("paused");
    let deliveries = Arc::new(Deliveries {
        records: Mutex::new(HashMap::from([(
            pending.entry_id.clone(),
            vec![EntryDeliveryRecord {
                entry_id: pending.entry_id.clone(),
                target_device_id: target.clone(),
                status: EntryDeliveryStatus::Pending,
                reason_detail: None,
                updated_at_ms: 1,
            }],
        )])),
    });
    let delivery = Arc::new(RecordingDispatch {
        commands: Mutex::new(Vec::new()),
        result: DispatchResult::Delivered,
    });
    let mut deps = recovery_deps(
        true,
        vec![pending.clone()],
        HashMap::from([(pending.event_id.clone(), DeviceId::new("local"))]),
        Arc::clone(&deliveries),
        Arc::clone(&delivery),
    );
    deps.member_scope = Arc::new(FixedMemberScope {
        usable: Vec::new(),
        paused: vec![PausedSpaceMember {
            device_id: target.clone(),
            reason: SpaceMemberPauseReason::RelationshipUnconfirmed,
        }],
    });

    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;

    assert!(delivery.commands.lock().unwrap().is_empty());
    let stored = deliveries.list_by_entry(&pending.entry_id).await.unwrap();
    assert!(matches!(
        stored.as_slice(),
        [record]
            if record.target_device_id == target
                && record.status == EntryDeliveryStatus::Pending
    ));
}

#[tokio::test]
async fn recovery_only_dispatches_the_newest_unreachable_entry_for_a_device() {
    let newest_entry = entry("newest-entry", "newest-event");
    let older_entry = entry("older-entry", "older-event");
    let target = DeviceId::new("recovered");
    let deliveries = Arc::new(Deliveries {
        records: Mutex::new(HashMap::from([
            (
                newest_entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: newest_entry.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Unreachable,
                    reason_detail: None,
                    updated_at_ms: 2,
                }],
            ),
            (
                older_entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: older_entry.entry_id.clone(),
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
        vec![newest_entry.clone(), older_entry.clone()],
        HashMap::from([
            (newest_entry.event_id.clone(), DeviceId::new("local")),
            (older_entry.event_id.clone(), DeviceId::new("local")),
        ]),
        Arc::clone(&deliveries),
        Arc::clone(&delivery),
    );

    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;

    let commands = delivery.commands.lock().unwrap();
    assert_eq!(
        commands.as_slice(),
        &[(newest_entry.entry_id.clone(), vec![target.clone()])]
    );
    drop(commands);

    let older_records = deliveries
        .list_by_entry(&older_entry.entry_id)
        .await
        .unwrap();
    assert!(
        !is_recovery_eligible(&older_records[0], &target),
        "an older offline entry must be replaced by the newest one"
    );
}

#[tokio::test]
async fn recovery_never_falls_back_to_an_older_offline_entry_after_a_newer_result() {
    let newest_entry = entry("newest-entry", "newest-event");
    let older_entry = entry("older-entry", "older-event");
    let target = DeviceId::new("recovered");
    let deliveries = Arc::new(Deliveries {
        records: Mutex::new(HashMap::from([
            (
                newest_entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: newest_entry.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Delivered,
                    reason_detail: None,
                    updated_at_ms: 2,
                }],
            ),
            (
                older_entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: older_entry.entry_id.clone(),
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
        vec![newest_entry.clone(), older_entry.clone()],
        HashMap::from([
            (newest_entry.event_id.clone(), DeviceId::new("local")),
            (older_entry.event_id.clone(), DeviceId::new("local")),
        ]),
        Arc::clone(&deliveries),
        Arc::clone(&delivery),
    );

    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;

    assert!(delivery.commands.lock().unwrap().is_empty());
    let older_records = deliveries
        .list_by_entry(&older_entry.entry_id)
        .await
        .unwrap();
    assert!(matches!(
        older_records[0].status,
        EntryDeliveryStatus::Superseded
    ));
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

    recover_for_target(&deps, target, &CancellationToken::new()).await;

    assert!(delivery.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn disabled_global_sync_never_dispatches_a_saved_offline_delivery() {
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
    let mut deps = recovery_deps(
        true,
        vec![pending_entry.clone()],
        HashMap::from([(pending_entry.event_id.clone(), DeviceId::new("local"))]),
        deliveries,
        Arc::clone(&delivery),
    );
    deps.settings = Arc::new(FixedSettings {
        sync_enabled: false,
        auto_sync_enabled: true,
    });

    recover_for_target(&deps, target, &CancellationToken::new()).await;

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

    recover_for_target(&deps, target.clone(), &CancellationToken::new()).await;

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
