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

use std::collections::VecDeque;

use tokio::sync::Notify;

use super::schedule::RETRY_BACKOFF;
use crate::clipboard::sync::dispatch_entry::test_support as dispatch_test_support;
use crate::clipboard::sync::dispatch_entry::RecordingOwner;
use crate::clipboard::sync::DispatchClipboardEntryUseCase;
use crate::deps::ConnectivityError;
use crate::deps::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    PausedSpaceMember, SpaceMemberPauseReason,
};
use crate::facade::HostEventBus;
use crate::transfer::progress::ProgressProjectionWorker;
use uc_core::clipboard::{ClipboardEntry, ClipboardRepositoryError};
use uc_core::ids::{EntryId, EventId};
use uc_core::ports::peer_reachability::{PeerReachabilityChanged, PeerReachabilityError};
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, ConnectionChannel, DispatchAck,
    DispatchReport, SyncPayload,
};
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
            recover_online_target(&deps, target, RecoveryTrigger::PeerOnline, &task_cancel).await;
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
        recover_online_target(
            &deps,
            DeviceId::new("target"),
            RecoveryTrigger::PeerOnline,
            &task_cancel,
        )
        .await;
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
    StorageFailure,
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
                reconciled: Vec::new(),
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
            DispatchResult::StorageFailure => Err(ResendEntryError::Storage(anyhow::anyhow!(
                "PRIVATE_STORAGE_DETAIL"
            ))),
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
        delivery_results: DeliveryResultFeed::new(),
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
async fn a_dispatch_failure_is_recorded_with_its_fixed_variant_and_no_error_text() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
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
        result: DispatchResult::StorageFailure,
    });
    let deps = recovery_deps(
        true,
        vec![pending_entry.clone()],
        HashMap::from([(pending_entry.event_id.clone(), DeviceId::new("local"))]),
        Arc::clone(&deliveries),
        delivery,
    );

    recover_for_target(&deps, target, &CancellationToken::new()).await;

    assert_eq!(logs.count("clipboard delivery recovery skipped entry"), 1);
    assert!(logs.output().contains("reason=\"storage\""));
    assert!(logs.output().contains("WARN"));
    assert!(!logs.output().contains("PRIVATE"));
}

#[tokio::test]
async fn payload_lost_stops_future_automatic_recovery_for_that_entry() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
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
    assert_eq!(logs.count("entry payload is gone"), 1);
    assert!(logs.output().contains("reason=\"payload_lost\""));
}

struct InactiveScope;

#[async_trait]
impl CurrentSpaceMemberScopePort for InactiveScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: false,
            usable_peer_device_ids: Vec::new(),
            paused_peer_devices: Vec::new(),
        })
    }
}

struct UnavailableScope;

#[async_trait]
impl CurrentSpaceMemberScopePort for UnavailableScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Err(CurrentSpaceMemberScopeError::Unavailable)
    }
}

/// 每一轮恢复都以一条固定结局记录收尾；提前返回不再静默（旧实现里关闭同步、成员失效、
/// 成员范围读取失败等分支什么都不留下）。
#[tokio::test]
async fn every_recovery_round_ends_with_one_fixed_outcome_record() {
    #[derive(Clone, Copy)]
    enum Setup {
        Normal,
        NoEntries,
        AutoSyncOff,
        Paused,
        NotCurrent,
        MemberInactive,
        ScopeUnavailable,
        PreCancelled,
        PayloadLost,
        StorageFailure,
    }
    let target = DeviceId::new("recovered");
    for (setup, expected) in [
        (Setup::Normal, "delivered"),
        (Setup::NoEntries, "nothing_to_recover"),
        (Setup::AutoSyncOff, "sync_disabled"),
        (Setup::Paused, "target_paused"),
        (Setup::NotCurrent, "target_not_current"),
        (Setup::MemberInactive, "member_inactive"),
        (Setup::ScopeUnavailable, "scope_unavailable"),
        (Setup::PreCancelled, "cancelled"),
        (Setup::PayloadLost, "payload_lost"),
        (Setup::StorageFailure, "delivery_failed"),
    ] {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let pending = entry("offline-entry", "local-event");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::from([(
                pending.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: pending.entry_id.clone(),
                    target_device_id: target.clone(),
                    status: EntryDeliveryStatus::Unreachable,
                    reason_detail: None,
                    updated_at_ms: 1,
                }],
            )])),
        });
        let delivery = Arc::new(RecordingDispatch {
            commands: Mutex::new(Vec::new()),
            result: match setup {
                Setup::PayloadLost => DispatchResult::PayloadLost,
                Setup::StorageFailure => DispatchResult::StorageFailure,
                _ => DispatchResult::Delivered,
            },
        });
        let entries = if matches!(setup, Setup::NoEntries) {
            Vec::new()
        } else {
            vec![pending.clone()]
        };
        let mut deps = recovery_deps(
            !matches!(setup, Setup::AutoSyncOff),
            entries,
            HashMap::from([(pending.event_id.clone(), DeviceId::new("local"))]),
            deliveries,
            delivery,
        );
        match setup {
            Setup::Paused => {
                deps.member_scope = Arc::new(FixedMemberScope {
                    usable: Vec::new(),
                    paused: vec![PausedSpaceMember {
                        device_id: target.clone(),
                        reason: SpaceMemberPauseReason::RelationshipUnconfirmed,
                    }],
                })
            }
            Setup::NotCurrent => {
                deps.member_scope = Arc::new(FixedMemberScope {
                    usable: Vec::new(),
                    paused: Vec::new(),
                })
            }
            Setup::MemberInactive => deps.member_scope = Arc::new(InactiveScope),
            Setup::ScopeUnavailable => deps.member_scope = Arc::new(UnavailableScope),
            _ => {}
        }
        let cancel = CancellationToken::new();
        if matches!(setup, Setup::PreCancelled) {
            cancel.cancel();
        }

        recover_online_target(&deps, target.clone(), RecoveryTrigger::PeerOnline, &cancel).await;

        let rounds: Vec<_> = logs
            .output()
            .lines()
            .filter(|line| line.contains("clipboard delivery recovery round finished"))
            .map(str::to_owned)
            .collect();
        assert_eq!(rounds.len(), 1, "{expected}: {}", logs.output());
        assert!(
            rounds[0].contains(&format!("outcome=\"{expected}\"")),
            "expected {expected}: {}",
            rounds[0]
        );
        assert!(
            rounds[0].contains("trigger=\"peer_online\""),
            "{}",
            rounds[0]
        );
        assert!(!logs.output().contains("PRIVATE"));
    }
}

// ── 设备保持在线时的有界退避重试 ──────────────────────────────────────

/// 可控的在线状态：测试中途改为离线时不发事件，模拟“状态已变但事件未到”。
struct SwitchablePeerReachability {
    state: Mutex<ReachabilityState>,
    tx: tokio::sync::broadcast::Sender<PeerReachabilityChanged>,
}

impl SwitchablePeerReachability {
    fn new(state: ReachabilityState) -> Arc<Self> {
        let (tx, _) = tokio::sync::broadcast::channel(8);
        Arc::new(Self {
            state: Mutex::new(state),
            tx,
        })
    }

    fn set(&self, state: ReachabilityState) {
        *self.state.lock().unwrap() = state;
    }

    fn announce(&self, device_id: DeviceId, state: ReachabilityState) {
        self.set(state);
        let _ = self.tx.send(PeerReachabilityChanged {
            device_id,
            state,
            at: chrono::Utc::now(),
        });
    }
}

#[async_trait]
impl PeerReachabilityPort for SwitchablePeerReachability {
    async fn ensure_reachable(
        &self,
        _device: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        Ok(*self.state.lock().unwrap())
    }

    async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
        *self.state.lock().unwrap()
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PeerReachabilityChanged> {
        self.tx.subscribe()
    }
}

/// 一次补发的结果；可恢复与成功结果都经真实记录负责人落库并发布。
#[derive(Clone, Copy)]
enum Attempt {
    /// 内容拨号失败或流在确认前中断，落为 `Unreachable`。
    Interrupted,
    /// 对端确认收下。
    Accepted,
    /// 上次确认丢失但对端已保存，这次得到重复确认。
    Duplicate,
    /// 补发前存储失败，没有落下任何投递结果。
    StorageFailure,
}

struct ScriptedDelivery {
    recorder: Arc<RecordingOwner>,
    attempts: Mutex<VecDeque<Attempt>>,
    calls: AtomicUsize,
}

impl ScriptedDelivery {
    fn new(recorder: Arc<RecordingOwner>, attempts: &[Attempt]) -> Arc<Self> {
        Arc::new(Self {
            recorder,
            attempts: Mutex::new(attempts.iter().copied().collect()),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RecoveryDeliveryPort for ScriptedDelivery {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let attempt = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra recovery attempt");
        let (status, report) = match attempt {
            Attempt::Interrupted => (
                EntryDeliveryStatus::Unreachable,
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 0,
                    duplicate: 0,
                    offline: 1,
                    errored: 0,
                    pending: 0,
                },
            ),
            Attempt::Accepted => (
                EntryDeliveryStatus::Delivered,
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 1,
                    duplicate: 0,
                    offline: 0,
                    errored: 0,
                    pending: 0,
                },
            ),
            Attempt::Duplicate => (
                EntryDeliveryStatus::Duplicate,
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 0,
                    duplicate: 1,
                    offline: 0,
                    errored: 0,
                    pending: 0,
                },
            ),
            Attempt::StorageFailure => {
                return Err(ResendEntryError::Storage(anyhow::anyhow!(
                    "PRIVATE_STORAGE_DETAIL"
                )))
            }
        };
        let records: Vec<_> = targets
            .into_iter()
            .map(|target_device_id| EntryDeliveryRecord {
                entry_id: entry_id.clone(),
                target_device_id,
                status: status.clone(),
                reason_detail: None,
                updated_at_ms: 43,
            })
            .collect();
        self.recorder.flush(&records).await;
        Ok(report)
    }
}

struct RetryHarness {
    recovery: OfflineDeliveryRecovery,
    recorder: Arc<RecordingOwner>,
    deliveries: Arc<Deliveries>,
    delivery: Arc<ScriptedDelivery>,
    reachability: Arc<SwitchablePeerReachability>,
    entry: ClipboardEntry,
    target: DeviceId,
}

impl RetryHarness {
    fn start(initial: Option<EntryDeliveryStatus>, attempts: &[Attempt]) -> Self {
        let entry = entry("copied-entry", "local-event");
        let target = DeviceId::new("target");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::new()),
        });
        if let Some(status) = initial {
            deliveries.records.lock().unwrap().insert(
                entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: entry.entry_id.clone(),
                    target_device_id: target,
                    status,
                    reason_detail: None,
                    updated_at_ms: 1,
                }],
            );
        }
        let recorder = Arc::new(RecordingOwner::new(
            Arc::clone(&deliveries) as Arc<dyn EntryDeliveryRepositoryPort>,
            Arc::new(HostEventBus::new()),
        ));
        let delivery = ScriptedDelivery::new(Arc::clone(&recorder), attempts);
        let reachability = SwitchablePeerReachability::new(ReachabilityState::Online);
        let mut deps = recovery_deps(
            true,
            vec![entry.clone()],
            HashMap::from([(entry.event_id.clone(), DeviceId::new("local"))]),
            Arc::clone(&deliveries),
            Arc::new(RecordingDispatch {
                commands: Mutex::new(Vec::new()),
                result: DispatchResult::Delivered,
            }),
        );
        deps.peer_reachability = Arc::clone(&reachability) as Arc<dyn PeerReachabilityPort>;
        deps.delivery = Arc::clone(&delivery) as Arc<dyn RecoveryDeliveryPort>;
        deps.delivery_results = recorder.delivery_results();
        Self {
            recovery: OfflineDeliveryRecovery::start(deps),
            recorder,
            deliveries,
            delivery,
            reachability,
            entry,
            target,
        }
    }

    /// 模拟自动投递：经真实记录负责人写入本次结果（并在可恢复时发布通知）。
    async fn record_dispatch(&self, status: EntryDeliveryStatus) {
        self.recorder
            .flush(&[EntryDeliveryRecord {
                entry_id: self.entry.entry_id.clone(),
                target_device_id: self.target,
                status,
                reason_detail: None,
                updated_at_ms: 2,
            }])
            .await;
    }

    fn status(&self) -> Option<EntryDeliveryStatus> {
        self.deliveries
            .records
            .lock()
            .unwrap()
            .get(&self.entry.entry_id)
            .and_then(|records| records.first())
            .map(|record| record.status.clone())
    }
}

/// 让后台恢复任务处理已经到达的通知与到期计时器。
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

const JUST_BEFORE: Duration = Duration::from_millis(1);

/// presence 一直在线、从未产生新的上线事件：内容拨号失败后按退避自动重试，
/// 第二次失败后间隔增长，第三次成功即停止，用户无需重新复制。
#[tokio::test(start_paused = true)]
async fn online_target_retries_a_failed_content_dial_without_a_new_online_event() {
    let harness = RetryHarness::start(None, &[Attempt::Interrupted, Attempt::Accepted]);
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;

    tokio::time::sleep(RETRY_BACKOFF[0] - JUST_BEFORE).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        0,
        "no retry before the first backoff"
    );
    tokio::time::sleep(JUST_BEFORE * 2).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    tokio::time::sleep(RETRY_BACKOFF[1] - JUST_BEFORE * 3).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        1,
        "second retry waits for a longer backoff"
    );
    tokio::time::sleep(JUST_BEFORE * 3).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));

    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        2,
        "a delivered target is not retried"
    );
    harness.recovery.shutdown().await.unwrap();
}

/// 流在确认前被重置（落为可恢复结果）后，重试得到重复确认即结束；
/// 接收端的去重保证确认丢失时不会重复写入。
#[tokio::test(start_paused = true)]
async fn lost_ack_retry_settles_as_duplicate_without_a_new_copy() {
    let harness = RetryHarness::start(None, &[Attempt::Duplicate]);
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;

    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    settle().await;

    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Duplicate));
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    harness.recovery.shutdown().await.unwrap();
}

/// 终态失败（策略拒绝、协议不兼容、不可重试错误）不会触发任何自动重试。
#[tokio::test(start_paused = true)]
async fn terminal_results_are_never_retried() {
    for reason in [
        DeliveryFailureReason::PeerRejected,
        DeliveryFailureReason::PeerIncompatible,
        DeliveryFailureReason::LocalPolicy,
        DeliveryFailureReason::Io,
        DeliveryFailureReason::Internal,
    ] {
        let harness = RetryHarness::start(None, &[]);
        settle().await;
        harness
            .record_dispatch(EntryDeliveryStatus::Failed { reason })
            .await;
        tokio::time::sleep(Duration::from_secs(600)).await;
        settle().await;
        assert_eq!(harness.delivery.calls(), 0);
        harness.recovery.shutdown().await.unwrap();
    }
}

/// 到期时目标已离线：不空转重试，改由下一次上线事件恢复。
#[tokio::test(start_paused = true)]
async fn due_retry_for_an_offline_target_waits_for_the_next_online_event() {
    let harness = RetryHarness::start(None, &[Attempt::Accepted]);
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;
    harness.reachability.set(ReachabilityState::Offline);

    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 0);

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 恢复轮次自身失败（补发前存储失败，没有新的投递结果）也按退避再试。
#[tokio::test(start_paused = true)]
async fn a_failed_recovery_round_is_retried_with_backoff() {
    let harness = RetryHarness::start(
        Some(EntryDeliveryStatus::Unreachable),
        &[Attempt::StorageFailure, Attempt::Accepted],
    );
    settle().await;
    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);

    tokio::time::sleep(RETRY_BACKOFF[0] - JUST_BEFORE).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    tokio::time::sleep(JUST_BEFORE * 2).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// presence 一直在线、每次补发都中断：同一条目只计时重试 3 次（约 2、5、10 秒后），
/// 第三次失败发布的可恢复结果不再排期，长时间内不再补发；之后只有新的上线事件才再补发一次。
#[tokio::test(start_paused = true)]
async fn an_online_target_gets_exactly_three_timed_retries_for_the_same_entry() {
    let harness = RetryHarness::start(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
    );
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;

    // 每次重试在上一结果到达后按下一个间隔到期；暂停时钟下结果在到期时刻即被处理。
    let mut due = Instant::now();
    for (retry, delay) in RETRY_BACKOFF.iter().enumerate() {
        due += *delay;
        tokio::time::sleep_until(due - JUST_BEFORE).await;
        settle().await;
        assert_eq!(
            harness.delivery.calls(),
            retry,
            "retry {retry} waits for its interval"
        );
        tokio::time::sleep_until(due + JUST_BEFORE).await;
        settle().await;
        assert_eq!(harness.delivery.calls(), retry + 1);
        assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));
    }

    tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        RETRY_BACKOFF.len(),
        "no timed retry after the third failed retry while the target stays online"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        RETRY_BACKOFF.len() + 1,
        "the next online event resends the newest undelivered entry once"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

// ── 经真实发送用例与记录负责人的端到端重试 ────────────────────────────

/// 下一次写入失败的种类；命中一次后存储即恢复。
#[derive(Clone, Copy, PartialEq, Eq)]
enum BrokenWrite {
    /// 发送前的 `Pending` 意图写入。
    Intent,
    /// 发送后的最终结果写入。
    Final,
}

/// 可注入一次写入失败的投递记录仓库；读取与其余写入照常经内存仓库。
struct FlakyDeliveries {
    inner: Deliveries,
    next_failure: Mutex<Option<BrokenWrite>>,
}

impl FlakyDeliveries {
    fn fail_next(&self, write: BrokenWrite) {
        *self.next_failure.lock().unwrap() = Some(write);
    }
}

#[async_trait]
impl EntryDeliveryRepositoryPort for FlakyDeliveries {
    async fn record_attempt(
        &self,
        record: &EntryDeliveryRecord,
    ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
        let write = if record.status == EntryDeliveryStatus::Pending {
            BrokenWrite::Intent
        } else {
            BrokenWrite::Final
        };
        {
            let mut next_failure = self.next_failure.lock().unwrap();
            if *next_failure == Some(write) {
                *next_failure = None;
                return Err(uc_core::clipboard::EntryDeliveryError::Storage {
                    source: anyhow::anyhow!("PRIVATE_STORAGE_DETAIL"),
                });
            }
        }
        self.inner.record_attempt(record).await
    }

    async fn list_by_entry(
        &self,
        entry_id: &EntryId,
    ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError> {
        self.inner.list_by_entry(entry_id).await
    }
}

/// 对端在一次投递中的表现。
#[derive(Clone, Copy)]
enum PeerStep {
    /// 确认前连接被重置，内容未保存。
    Interrupted,
    /// 收下内容；同一快照再次到达时按接收端去重返回重复确认。
    Receive,
    /// 经过给定时长才收下并确认，用于越过前台期限、进入后台收尾。
    ReceiveAfter(Duration),
    /// 取得连接时准入被拒绝（经共享拨号结果返回）。
    Denied,
}

/// 按脚本应答的对端：只在真正收下内容时保存一份副本。
struct ScriptedPeer {
    steps: Mutex<VecDeque<PeerStep>>,
    dials: AtomicUsize,
    stored: Mutex<Vec<String>>,
}

impl ScriptedPeer {
    fn new(steps: &[PeerStep]) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.iter().copied().collect()),
            dials: AtomicUsize::new(0),
            stored: Mutex::new(Vec::new()),
        })
    }

    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }

    fn copies(&self) -> usize {
        self.stored.lock().unwrap().len()
    }

    fn receive(&self, snapshot_hash: &str) -> DispatchAck {
        let mut stored = self.stored.lock().unwrap();
        if stored.iter().any(|hash| hash == snapshot_hash) {
            DispatchAck::DuplicateIgnored
        } else {
            stored.push(snapshot_hash.to_owned());
            DispatchAck::Accepted
        }
    }
}

#[async_trait]
impl ClipboardDispatchPort for ScriptedPeer {
    async fn dispatch(
        &self,
        _target: &DeviceId,
        header: &ClipboardHeader,
        _payload: SyncPayload,
    ) -> DispatchReport {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra delivery attempt");
        let outcome = match step {
            PeerStep::Interrupted => Err(ClipboardDispatchError::Transient(Box::new(
                io::Error::from(io::ErrorKind::ConnectionReset),
            ))),
            PeerStep::Receive => Ok(self.receive(&header.snapshot_hash)),
            PeerStep::ReceiveAfter(delay) => {
                tokio::time::sleep(delay).await;
                Ok(self.receive(&header.snapshot_hash))
            }
            PeerStep::Denied => Err(ClipboardDispatchError::Io(Box::new(
                ConnectivityError::Shared(Arc::new(ConnectivityError::Denied)),
            ))),
        };
        DispatchReport {
            transport: ConnectionChannel::Direct,
            outcome,
        }
    }
}

/// 恢复补发经真实发送用例；只省略从仓库重建快照这一步，快照哈希保持不变以便接收端去重。
struct RealDelivery(Arc<DispatchClipboardEntryUseCase>);

impl RealDelivery {
    async fn send(
        &self,
        entry_id: EntryId,
        targets: Option<Vec<DeviceId>>,
    ) -> Result<ResendReport, ResendEntryError> {
        let mut input = dispatch_test_support::dispatch_input();
        input.entry_id = Some(entry_id);
        input.target_filter = targets;
        let outcome = self
            .0
            .execute(input)
            .await
            .map_err(|error| ResendEntryError::Dispatch(anyhow::Error::new(error)))?;
        Ok(ResendReport {
            reconciled: Vec::new(),
            accepted: outcome.total_accepted,
            duplicate: outcome.total_duplicate,
            offline: outcome.total_offline,
            errored: outcome.total_errored,
            pending: outcome.total_pending,
        })
    }
}

#[async_trait]
impl RecoveryDeliveryPort for RealDelivery {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        self.send(entry_id, Some(targets)).await
    }
}

struct RealPathHarness {
    recovery: OfflineDeliveryRecovery,
    _progress_worker: ProgressProjectionWorker,
    delivery: Arc<RealDelivery>,
    deliveries: Arc<FlakyDeliveries>,
    peer: Arc<ScriptedPeer>,
    entry: ClipboardEntry,
    target: DeviceId,
}

impl RealPathHarness {
    fn start(steps: &[PeerStep]) -> Self {
        let entry = entry("copied-entry", "local-event");
        let target = DeviceId::new("target");
        let deliveries = Arc::new(FlakyDeliveries {
            inner: Deliveries {
                records: Mutex::new(HashMap::new()),
            },
            next_failure: Mutex::new(None),
        });
        let peer = ScriptedPeer::new(steps);
        // presence 始终在线，整个测试不产生任何上线事件。
        let reachability = SwitchablePeerReachability::new(ReachabilityState::Online);
        let scope = Arc::new(FixedMemberScope {
            usable: vec![target],
            paused: Vec::new(),
        });
        let (use_case, progress_worker) = dispatch_test_support::dispatch_use_case(
            &[target],
            Arc::clone(&peer) as Arc<dyn ClipboardDispatchPort>,
            Arc::clone(&deliveries) as Arc<dyn EntryDeliveryRepositoryPort>,
            Arc::clone(&reachability) as Arc<dyn PeerReachabilityPort>,
            Arc::clone(&scope) as Arc<dyn CurrentSpaceMemberScopePort>,
        );
        let use_case = Arc::new(use_case);
        let delivery = Arc::new(RealDelivery(Arc::clone(&use_case)));
        let mut deps = recovery_deps(
            true,
            vec![entry.clone()],
            HashMap::from([(entry.event_id.clone(), DeviceId::new("local"))]),
            Arc::new(Deliveries {
                records: Mutex::new(HashMap::new()),
            }),
            Arc::new(RecordingDispatch {
                commands: Mutex::new(Vec::new()),
                result: DispatchResult::Delivered,
            }),
        );
        deps.peer_reachability = reachability;
        deps.member_scope = scope;
        deps.deliveries = Arc::clone(&deliveries) as Arc<dyn EntryDeliveryRepositoryPort>;
        deps.delivery = Arc::clone(&delivery) as Arc<dyn RecoveryDeliveryPort>;
        deps.delivery_results = use_case.delivery_results();
        Self {
            recovery: OfflineDeliveryRecovery::start(deps),
            _progress_worker: progress_worker,
            delivery,
            deliveries,
            peer,
            entry,
            target,
        }
    }

    /// 本机复制后的自动发送。
    async fn copy(&self) {
        self.delivery
            .send(self.entry.entry_id.clone(), None)
            .await
            .unwrap();
    }

    /// 用户对同一条内容的手动重发。
    async fn resend(&self) {
        self.delivery
            .send(self.entry.entry_id.clone(), Some(vec![self.target]))
            .await
            .unwrap();
    }

    fn status(&self) -> Option<EntryDeliveryStatus> {
        self.deliveries
            .inner
            .records
            .lock()
            .unwrap()
            .get(&self.entry.entry_id)
            .and_then(|records| records.first())
            .map(|record| record.status.clone())
    }

    async fn shutdown(self) {
        self.recovery.shutdown().await.unwrap();
        self.delivery.0.shutdown().await.unwrap();
    }
}

/// 让发送、记录、后台收尾与恢复任务处理完已经就绪的工作。
async fn drain() {
    for _ in 0..256 {
        tokio::task::yield_now().await;
    }
}

/// 在线目标的重试轮次里 `Pending` 意图写入失败一次：恢复负责人仍按退避再试，
/// 存储恢复后送达，对端只保存一份副本。
#[tokio::test(start_paused = true)]
async fn a_failed_retry_intent_write_keeps_the_delivery_in_recovery() {
    let harness = RealPathHarness::start(&[PeerStep::Interrupted, PeerStep::Receive]);
    drain().await;
    harness.copy().await;
    drain().await;
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    harness.deliveries.fail_next(BrokenWrite::Intent);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        1,
        "a failed intent write sends nothing"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    tokio::time::sleep(RETRY_BACKOFF[1]).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);

    tokio::time::sleep(Duration::from_secs(600)).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2, "a delivered target is not retried");
    harness.shutdown().await;
}

/// 在线目标的重试轮次再次中断，且最终 `Unreachable` 写入失败一次（持久状态停在 `Pending`）：
/// 恢复负责人仍按退避再试，存储恢复后送达，对端只保存一份副本。
#[tokio::test(start_paused = true)]
async fn a_failed_retry_result_write_keeps_the_delivery_in_recovery() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::Interrupted,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;

    harness.deliveries.fail_next(BrokenWrite::Final);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Pending));

    tokio::time::sleep(RETRY_BACKOFF[1]).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 3);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}

/// 首次自动发送的最终写入失败（持久状态停在 `Pending`）：结果仍交给恢复负责人，
/// 无需新的上线事件或重新复制即可送达。
#[tokio::test(start_paused = true)]
async fn an_initial_send_whose_result_write_failed_still_reaches_recovery() {
    let harness = RealPathHarness::start(&[PeerStep::Interrupted, PeerStep::Receive]);
    drain().await;
    harness.deliveries.fail_next(BrokenWrite::Final);
    harness.copy().await;
    drain().await;
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Pending));

    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}

/// 重试以准入拒绝终结：落为终态失败、不再重试，且该目标的退避状态被清除。
/// 清除与否由之后同一内容的新可恢复结果的首个间隔区分：残留状态会沿用已增长的间隔。
#[tokio::test(start_paused = true)]
async fn a_retry_that_ends_terminally_clears_the_target_backoff() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::Denied,
        PeerStep::Interrupted,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(
        harness.status(),
        Some(EntryDeliveryStatus::Failed {
            reason: DeliveryFailureReason::Io
        })
    );
    tokio::time::sleep(Duration::from_secs(600)).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2, "a denied delivery is not retried");

    harness.resend().await;
    drain().await;
    assert_eq!(harness.peer.dials(), 3);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        4,
        "the next recoverable result starts from the first interval"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.shutdown().await;
}

/// 重试越过前台期限、随后在后台收尾中送达：该目标的退避状态被清除。
#[tokio::test(start_paused = true)]
async fn a_retry_completed_by_the_deferred_drain_clears_the_target_backoff() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::ReceiveAfter(Duration::from_secs(6)),
        PeerStep::Interrupted,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;
    tokio::time::sleep(RETRY_BACKOFF[0] + Duration::from_secs(6) + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);

    harness.resend().await;
    drain().await;
    assert_eq!(harness.peer.dials(), 3);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        4,
        "the next recoverable result starts from the first interval"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Duplicate));
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}
