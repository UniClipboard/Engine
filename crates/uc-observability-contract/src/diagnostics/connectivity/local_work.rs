//! 内部工作只记录本地等待与执行证据，不创建业务 span 或改变调用结果。
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::super::AdmissionObservationAction;
use super::maintenance::MaintenanceContext;
use super::{emit_local, millis, record::LocalEvent, AdmissionExchangeSide, ObservationContext};

tokio::task_local! {
    static PAIRING_WORK: PairingWork;
    static WORK_ACTIVE: ();
    static BLOB_PUBLISH_ACTIVE: ();
    static SECURE_STORAGE_TALLY: Arc<SecureStorageTally>;
}

/// 一项被观测工作内部的安全存储读取次数与累计耗时；子工作的读取计入外层，外层数值是含子工作的总和。
#[derive(Default)]
struct SecureStorageTally {
    reads: AtomicU64,
    nanos: AtomicU64,
}

impl SecureStorageTally {
    fn add(&self, reads: u64, nanos: u64) {
        self.reads.fetch_add(reads, Ordering::Relaxed);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    fn usage(&self) -> Option<SecureStorageUse> {
        let reads = self.reads.load(Ordering::Relaxed);
        (reads > 0).then(|| SecureStorageUse {
            reads,
            duration_ms: self.nanos.load(Ordering::Relaxed) / 1_000_000,
        })
    }

    fn add_into(&self, parent: &Self) {
        parent.add(
            self.reads.load(Ordering::Relaxed),
            self.nanos.load(Ordering::Relaxed),
        );
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SecureStorageUse {
    reads: u64,
    duration_ms: u64,
}

/// 记录一次安全存储读取；只在被观测的本地工作内累计，其余位置不产生任何记录。
pub fn record_secure_storage_read(elapsed: Duration) {
    let _ = SECURE_STORAGE_TALLY.try_with(|tally| {
        tally.add(1, u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    });
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PairingWork {
    side: AdmissionExchangeSide,
    message: Option<PairingMessage>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PairingMessage {
    JoinRequest,
    Prepared,
    Applied,
    CompleteAck,
    CancelRequested,
}

pub fn scope_pairing_work<T>(
    side: AdmissionExchangeSide,
    action: Option<AdmissionObservationAction>,
    work: impl Future<Output = T>,
) -> impl Future<Output = T> {
    // 在进入新 future 前固定存放业务 future，避免观测嵌套放大本机调用栈。
    let work = Box::pin(work);
    async move {
        let message = action.map(|action| match action {
            AdmissionObservationAction::RequestJoin => PairingMessage::JoinRequest,
            AdmissionObservationAction::ConfirmPrepared => PairingMessage::Prepared,
            AdmissionObservationAction::ConfirmApplied => PairingMessage::Applied,
            AdmissionObservationAction::Settle => PairingMessage::CompleteAck,
            AdmissionObservationAction::Cancel => PairingMessage::CancelRequested,
        });
        let context = ObservationContext::capture();
        context
            .scope(PAIRING_WORK.scope(PairingWork { side, message }, work))
            .await
    }
}

pub fn scope_blob_publish<T>(work: impl Future<Output = T>) -> impl Future<Output = T> {
    BLOB_PUBLISH_ACTIVE.scope((), work)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalWorkStep {
    ProtocolLock,
    RecoveryLock,
    SponsorStateLoad,
    SponsorStateCommit,
    SponsorPrepareCandidate,
    SponsorPrepareCommit,
    SponsorPrepareComplete,
    SponsorPrepareSettled,
    SponsorActivate,
    RePairingStateCommit,
    JoinerStateLoad,
    JoinerStateCommit,
    JoinerPrepareInvitation,
    JoinerResolveInvitation,
    JoinerPrepareStart,
    JoinerPrepareCandidate,
    JoinerPrepareApplied,
    JoinerPrepareActivation,
    JoinerActivate,
    JoinerProcessReply,
    RepositoryLoad,
    RepositorySave,
    MaintenanceLock,
    MaintenanceAdmissions,
    MaintenanceRestricted,
    MaintenanceEffects,
    MaintenanceConflicts,
    MaintenanceGroupUpdates,
    MaintenanceGroupUpdateDispatch,
    MaintenanceSynchronizationCheck,
    MaintenanceSynchronization,
    MaintenanceCleanup,
    SessionDrainOperations,
    SessionDrainGrace,
    SessionDrainCancellation,
    SessionStopTasks,
    SessionStopApplication,
    SessionStopNetwork,
    SessionCompleteTransition,
    SessionPrepare,
    SessionRecover,
    SessionStart,
    ClipboardFileSetResolve,
    ClipboardFileMetadataRead,
    ClipboardOutboundPlan,
    ClipboardLiveIndex,
    ClipboardDeliveryGateWait,
    ClipboardSyncSettingsLoad,
    ClipboardImageContentHash,
    ClipboardImagePayloadTake,
    BlobPlaintextHash,
    BlobCompress,
    BlobEncrypt,
    BlobStorePublish,
    BlobReferenceSave,
    BlobTicketIssue,
    SourceSnapshotLoad,
    DatabaseConnectionAcquire,
    JoinerResolveInvitationCloud,
    JoinerResolveInvitationLan,
    SpaceTransitionAdvance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalWorkOutcome {
    Ok,
    Error,
    Deferred,
    Rejected,
    Corrupt,
    Interrupted,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum LocalWorkEvent {
    Started {
        step: LocalWorkStep,
        pairing: Option<PairingWork>,
        maintenance: Option<MaintenanceContext>,
    },
    Finished {
        step: LocalWorkStep,
        pairing: Option<PairingWork>,
        maintenance: Option<MaintenanceContext>,
        duration_ms: u64,
        outcome: LocalWorkOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secure_storage: Option<SecureStorageUse>,
    },
}

impl LocalWorkEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Started { .. } => "runtime.work.started",
            Self::Finished { .. } => "runtime.work.finished",
        }
    }

    pub(super) fn level(&self) -> &'static str {
        match self {
            Self::Finished {
                outcome: LocalWorkOutcome::Corrupt,
                ..
            } => "ERROR",
            Self::Finished {
                outcome:
                    LocalWorkOutcome::Error | LocalWorkOutcome::Rejected | LocalWorkOutcome::Interrupted,
                ..
            } => "WARN",
            _ => "INFO",
        }
    }

    pub(super) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        let (step, pairing, maintenance) = match self {
            Self::Started {
                step,
                pairing,
                maintenance,
            }
            | Self::Finished {
                step,
                pairing,
                maintenance,
                ..
            } => (step, pairing, maintenance),
        };
        if let Some(maintenance) = maintenance {
            maintenance.fields(&mut fields);
        }
        fields.insert("step".into(), json!(step));
        if let Some(pairing) = pairing {
            fields.insert("uc.role".into(), json!(pairing.side));
            if let Some(message) = pairing.message {
                fields.insert("message".into(), json!(message));
                let round = match message {
                    PairingMessage::JoinRequest => Some(1),
                    PairingMessage::Prepared => Some(2),
                    PairingMessage::Applied => Some(3),
                    PairingMessage::CompleteAck => Some(4),
                    PairingMessage::CancelRequested => None,
                };
                if let Some(round) = round {
                    fields.insert("protocol_round".into(), json!(round));
                }
            }
        }
        if let Self::Finished {
            duration_ms,
            outcome,
            secure_storage,
            ..
        } = self
        {
            fields.insert("duration_ms".into(), json!(duration_ms));
            fields.insert("uc.outcome".into(), json!(outcome));
            if let Some(usage) = secure_storage {
                fields.insert("secure_storage_reads".into(), json!(usage.reads));
                fields.insert("secure_storage_read_ms".into(), json!(usage.duration_ms));
            }
        }
        fields
    }
}

pub struct LocalWorkObservation {
    step: LocalWorkStep,
    context: ObservationContext,
    started: Instant,
    finished: bool,
    pairing: Option<PairingWork>,
    maintenance: Option<MaintenanceContext>,
    enabled: bool,
    tally: Option<Arc<SecureStorageTally>>,
}

impl LocalWorkObservation {
    pub fn begin(step: LocalWorkStep) -> Self {
        let observation = Self {
            step,
            context: ObservationContext::capture(),
            started: Instant::now(),
            finished: false,
            pairing: PAIRING_WORK.try_with(|value| *value).ok(),
            maintenance: MaintenanceContext::capture(),
            enabled: !matches!(step, LocalWorkStep::ProtocolLock)
                || PAIRING_WORK.try_with(|_| ()).is_ok(),
            tally: None,
        };
        observation.emit(LocalWorkEvent::Started {
            step,
            pairing: observation.pairing,
            maintenance: observation.maintenance,
        });
        observation
    }

    pub fn finish(mut self, outcome: LocalWorkOutcome) {
        self.finished = true;
        self.emit(self.finished_event(outcome));
    }

    fn finished_event(&self, outcome: LocalWorkOutcome) -> LocalWorkEvent {
        LocalWorkEvent::Finished {
            step: self.step,
            pairing: self.pairing,
            maintenance: self.maintenance,
            duration_ms: millis(self.started.elapsed()),
            outcome,
            secure_storage: self.tally.as_deref().and_then(SecureStorageTally::usage),
        }
    }

    /// 让本项工作累计内部的安全存储读取，结束时随完成记录输出。
    fn with_tally(mut self, tally: Arc<SecureStorageTally>) -> Self {
        self.tally = Some(tally);
        self
    }

    fn emit(&self, record: LocalWorkEvent) {
        if !self.enabled {
            return;
        }
        emit_local(LocalEvent::LocalWork { record }, &self.context);
    }
}

impl Drop for LocalWorkObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(self.finished_event(LocalWorkOutcome::Interrupted));
        }
    }
}

pub fn observe_local_result<T, E>(
    step: LocalWorkStep,
    work: impl Future<Output = Result<T, E>>,
) -> impl Future<Output = Result<T, E>> {
    // 与会话观测一致，不能让计时包裹复制大型业务 future 的内联状态。
    let work = Box::pin(work);
    async move {
        let tally = Arc::new(SecureStorageTally::default());
        let parent = SECURE_STORAGE_TALLY.try_with(Arc::clone).ok();
        let observation = LocalWorkObservation::begin(step).with_tally(Arc::clone(&tally));
        let result = SECURE_STORAGE_TALLY
            .scope(Arc::clone(&tally), WORK_ACTIVE.scope((), work))
            .await;
        observation.finish(if result.is_ok() {
            LocalWorkOutcome::Ok
        } else {
            LocalWorkOutcome::Error
        });
        if let Some(parent) = parent {
            tally.add_into(&parent);
        }
        result
    }
}

pub fn observe_local_sync_result<T, E>(
    step: LocalWorkStep,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if WORK_ACTIVE.try_with(|_| ()).is_err() {
        return work();
    }
    let tally = Arc::new(SecureStorageTally::default());
    let parent = SECURE_STORAGE_TALLY.try_with(Arc::clone).ok();
    let observation = LocalWorkObservation::begin(step).with_tally(Arc::clone(&tally));
    let result = SECURE_STORAGE_TALLY.sync_scope(Arc::clone(&tally), work);
    observation.finish(if result.is_ok() {
        LocalWorkOutcome::Ok
    } else {
        LocalWorkOutcome::Error
    });
    if let Some(parent) = parent {
        tally.add_into(&parent);
    }
    result
}

/// 只在配对工作范围内记录一次同步等待；范围外的热路径不付任何观测成本。
pub fn observe_pairing_wait<T, E>(
    step: LocalWorkStep,
    wait: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if PAIRING_WORK.try_with(|_| ()).is_err() {
        return wait();
    }
    let observation = LocalWorkObservation::begin(step);
    let result = wait();
    observation.finish(if result.is_ok() {
        LocalWorkOutcome::Ok
    } else {
        LocalWorkOutcome::Error
    });
    result
}

pub fn observe_blob_publish_sync_result<T, E>(
    step: LocalWorkStep,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if BLOB_PUBLISH_ACTIVE.try_with(|_| ()).is_err() {
        return work();
    }
    let observation = LocalWorkObservation::begin(step);
    let result = work();
    observation.finish(if result.is_ok() {
        LocalWorkOutcome::Ok
    } else {
        LocalWorkOutcome::Error
    });
    result
}

#[cfg(test)]
mod tests {
    use super::super::decode_local_record;
    use super::*;

    #[test]
    fn local_work_rejects_uncontrolled_fields_and_inconsistent_severity() {
        let valid = json!({"kind": "local_work", "record": {"event": "finished", "step": "sponsor_state_load", "pairing": {"side": "sponsor", "message": "join_request"}, "maintenance": null, "duration_ms": 123, "outcome": "error"}});
        let name = "runtime.work.finished";
        assert!(decode_local_record(name, &valid.to_string(), "WARN").is_some());
        assert!(decode_local_record(name, &valid.to_string(), "INFO").is_none());
        assert!(decode_local_record("runtime.work.started", &valid.to_string(), "WARN").is_none());
        for (field, value) in [
            ("step", json!("PRIVATE")),
            ("outcome", json!("PRIVATE")),
            ("duration_ms", json!(-1)),
            ("path", json!("PRIVATE")),
            ("pairing", json!({"side": "joiner", "message": "PRIVATE"})),
            ("maintenance", json!({"round": 1, "trigger": "PRIVATE"})),
        ] {
            let mut invalid = valid.clone();
            invalid["record"][field] = value;
            assert!(decode_local_record(name, &invalid.to_string(), "WARN").is_none());
        }
    }

    #[test]
    fn clipboard_outbound_steps_are_accepted_by_the_closed_local_contract() {
        for step in [
            "clipboard_file_set_resolve",
            "clipboard_file_metadata_read",
            "clipboard_outbound_plan",
            "clipboard_live_index",
            "clipboard_delivery_gate_wait",
            "clipboard_sync_settings_load",
            "clipboard_image_content_hash",
            "clipboard_image_payload_take",
            "blob_plaintext_hash",
            "blob_compress",
            "blob_encrypt",
            "blob_store_publish",
            "blob_reference_save",
            "blob_ticket_issue",
        ] {
            let record = json!({
                "kind": "local_work",
                "record": {
                    "event": "finished",
                    "step": step,
                    "pairing": null,
                    "maintenance": null,
                    "duration_ms": 12,
                    "outcome": "ok"
                }
            });
            let decoded = decode_local_record("runtime.work.finished", &record.to_string(), "INFO")
                .expect("blob publish step should be accepted");
            assert_eq!(decoded["step"], step);
        }
    }

    #[test]
    fn secure_storage_use_and_new_pairing_steps_are_part_of_the_closed_contract() {
        let name = "runtime.work.finished";
        let with_use = json!({"kind": "local_work", "record": {"event": "finished", "step": "repository_save", "pairing": null, "maintenance": null, "duration_ms": 12, "outcome": "ok", "secure_storage": {"reads": 7, "duration_ms": 9}}});
        let decoded = decode_local_record(name, &with_use.to_string(), "INFO")
            .expect("secure storage use should be accepted");
        assert_eq!(decoded["secure_storage_reads"], 7);
        assert_eq!(decoded["secure_storage_read_ms"], 9);
        let mut extra = with_use.clone();
        extra["record"]["secure_storage"]["key_name"] = json!("PRIVATE");
        assert!(decode_local_record(name, &extra.to_string(), "INFO").is_none());
        for step in [
            "source_snapshot_load",
            "database_connection_acquire",
            "joiner_resolve_invitation_cloud",
            "joiner_resolve_invitation_lan",
            "space_transition_advance",
        ] {
            let record = json!({"kind": "local_work", "record": {"event": "finished", "step": step, "pairing": null, "maintenance": null, "duration_ms": 1, "outcome": "ok"}});
            assert!(
                decode_local_record(name, &record.to_string(), "INFO").is_some(),
                "{step}"
            );
        }
    }
}
