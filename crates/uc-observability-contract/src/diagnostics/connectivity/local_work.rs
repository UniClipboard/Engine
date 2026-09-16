//! 内部工作只记录本地等待与执行证据，不创建业务 span 或改变调用结果。
use std::future::Future;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::super::AdmissionObservationAction;
use super::maintenance::MaintenanceContext;
use super::{emit_local, millis, record::LocalEvent, AdmissionExchangeSide, ObservationContext};

tokio::task_local! {
    static PAIRING_WORK: PairingWork;
    static WORK_ACTIVE: ();
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
            ..
        } = self
        {
            fields.insert("duration_ms".into(), json!(duration_ms));
            fields.insert("uc.outcome".into(), json!(outcome));
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
        self.emit(LocalWorkEvent::Finished {
            step: self.step,
            pairing: self.pairing,
            maintenance: self.maintenance,
            duration_ms: millis(self.started.elapsed()),
            outcome,
        });
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
            self.emit(LocalWorkEvent::Finished {
                step: self.step,
                pairing: self.pairing,
                maintenance: self.maintenance,
                duration_ms: millis(self.started.elapsed()),
                outcome: LocalWorkOutcome::Interrupted,
            });
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
        let observation = LocalWorkObservation::begin(step);
        let result = WORK_ACTIVE.scope((), work).await;
        observation.finish(if result.is_ok() {
            LocalWorkOutcome::Ok
        } else {
            LocalWorkOutcome::Error
        });
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
}
