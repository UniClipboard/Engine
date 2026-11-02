//! 仅验收 feature 启用的负责人本地有界事件记录；不参与恢复决定。
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use serde::Serialize;
use tokio::time::Instant;
use uc_core::clipboard::EntryDeliveryRecord;
use uc_core::ids::{DeviceId, EntryId};
use uc_observability_contract::wire_enum;
use uuid::Uuid;

const EVENT_LIMIT: usize = 1024;
const ITEM_LIMIT: usize = 256;
const TARGET_LIMIT: usize = 64;

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, Serialize)]
    pub enum RecoveryEventKind {
        ResultRecoverable => "result_recoverable",
        ResultNotPersisted => "result_not_persisted",
        ResultSettled => "result_settled",
        ResultWriteFenced => "result_write_fenced",
        Reconciliation => "reconciliation",
        RoundFailure => "round_failure",
        ScheduleUpdated => "schedule_updated",
        RetryDue => "retry_due",
        RoundStarted => "round_started",
        DispatchStarted => "dispatch_started",
        RoundFinished => "round_finished",
        AwaitingResult => "awaiting_result",
        SettlementApplied => "settlement_applied",
        SettlementIgnored => "settlement_ignored",
        Cleared => "cleared",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RecoveryEventRef {
    pub epoch_alias: Uuid,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryEvent {
    pub seq: u64,
    pub at_ns: u64,
    pub kind: RecoveryEventKind,
    pub target_alias: Uuid,
    pub item_alias: Option<Uuid>,
    pub tracked_item_alias: Option<Uuid>,
    pub rounds_started: usize,
    pub due_ns: Option<u64>,
    pub schedule_origin: Option<RecoveryEventRef>,
    pub cause: Option<RecoveryEventRef>,
    pub trigger: Option<&'static str>,
    pub outcome: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoverySnapshot {
    pub epoch_alias: Uuid,
    pub now_ns: u64,
    pub events: Vec<RecoveryEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryObservationError {
    #[error("recovery observation has not started")]
    Disabled,
    #[error("recovery observation overflow invalidated this epoch")]
    Overflow,
}

struct State {
    epoch: Uuid,
    origin: Instant,
    enabled: bool,
    overflow: bool,
    targets: HashMap<DeviceId, Uuid>,
    items: HashMap<(DeviceId, EntryId), Uuid>,
    events: Vec<RecoveryEvent>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            epoch: Uuid::new_v4(),
            origin: Instant::now(),
            enabled: false,
            overflow: false,
            targets: HashMap::new(),
            items: HashMap::new(),
            events: Vec::new(),
        }
    }
}

impl State {
    fn item_alias(&mut self, target: DeviceId, entry: Option<&EntryId>) -> Option<Uuid> {
        let entry = entry?;
        let key = (target, entry.clone());
        if !self.items.contains_key(&key) && self.items.len() >= ITEM_LIMIT {
            self.overflow = true;
            return None;
        }
        Some(*self.items.entry(key).or_insert_with(Uuid::new_v4))
    }

    fn time(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.origin)
            .as_nanos()
            .min(u64::MAX as u128) as u64
    }
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 只在新的空闲验收场景开始前调用；不改变生产排期或持久状态。
pub fn begin_recovery_observation() -> Uuid {
    let mut state = state();
    *state = State {
        enabled: true,
        ..State::default()
    };
    state.epoch
}

/// 溢出后拒绝给出可用于通过验收的快照；必须重建场景后开始新 epoch。
pub fn recovery_snapshot() -> Result<RecoverySnapshot, RecoveryObservationError> {
    let state = state();
    if !state.enabled {
        return Err(RecoveryObservationError::Disabled);
    }
    if state.overflow {
        return Err(RecoveryObservationError::Overflow);
    }
    Ok(RecoverySnapshot {
        epoch_alias: state.epoch,
        now_ns: state.time(Instant::now()),
        events: state.events.clone(),
    })
}

/// 调度器的测试专属快照；不包含任何业务身份。
#[derive(Clone, Copy, Default)]
pub(crate) struct ScheduleObservation {
    pub(crate) rounds: usize,
    pub(crate) due: Option<Instant>,
    pub(crate) origin: Option<RecoveryEventRef>,
}

pub(crate) struct EventInput<'a> {
    pub(crate) kind: RecoveryEventKind,
    pub(crate) target: DeviceId,
    pub(crate) entry: Option<&'a EntryId>,
    pub(crate) tracked_entry: Option<&'a EntryId>,
    pub(crate) schedule: ScheduleObservation,
    pub(crate) at: Instant,
    pub(crate) trigger: Option<&'static str>,
    pub(crate) outcome: Option<&'static str>,
    pub(crate) cause: Option<RecoveryEventRef>,
}

pub(crate) fn record(input: EventInput<'_>) -> Option<RecoveryEventRef> {
    let mut state = state();
    if !state.enabled || state.overflow {
        return None;
    }
    if state.events.len() >= EVENT_LIMIT
        || (!state.targets.contains_key(&input.target) && state.targets.len() >= TARGET_LIMIT)
        || input.entry.is_some_and(|entry| {
            !state.items.contains_key(&(input.target, entry.clone()))
                && state.items.len() >= ITEM_LIMIT
        })
    {
        state.overflow = true;
        return None;
    }
    let target_alias = *state
        .targets
        .entry(input.target)
        .or_insert_with(Uuid::new_v4);
    let item_alias = state.item_alias(input.target, input.entry);
    let tracked_item_alias = state.item_alias(input.target, input.tracked_entry);
    if state.overflow {
        return None;
    }
    let seq = state.events.len() as u64 + 1;
    let event = RecoveryEvent {
        seq,
        at_ns: state.time(input.at),
        kind: input.kind,
        target_alias,
        item_alias,
        tracked_item_alias,
        rounds_started: input.schedule.rounds,
        due_ns: input.schedule.due.map(|due| state.time(due)),
        schedule_origin: input.schedule.origin,
        cause: input.cause,
        trigger: input.trigger,
        outcome: input.outcome,
    };
    state.events.push(event);
    Some(RecoveryEventRef {
        epoch_alias: state.epoch,
        seq,
    })
}

pub(crate) struct RoundObservation {
    pub(crate) schedule: ScheduleObservation,
    pub(crate) trigger: &'static str,
}

tokio::task_local! {
    pub(crate) static ROUND_SCHEDULE: RoundObservation;
}

pub(crate) fn round_event(
    kind: RecoveryEventKind,
    target: DeviceId,
    entry: Option<&EntryId>,
    trigger: Option<&'static str>,
    outcome: Option<&'static str>,
) {
    let (schedule, context_trigger) = ROUND_SCHEDULE
        .try_with(|context| (context.schedule, Some(context.trigger)))
        .unwrap_or_default();
    record(EventInput {
        kind,
        target,
        entry,
        tracked_entry: None,
        schedule,
        at: Instant::now(),
        trigger: trigger.or(context_trigger),
        outcome,
        cause: None,
    });
}

pub(crate) fn record_fenced_result(result: &EntryDeliveryRecord) {
    record(EventInput {
        kind: RecoveryEventKind::ResultWriteFenced,
        target: result.target_device_id,
        entry: Some(&result.entry_id),
        tracked_entry: None,
        schedule: ScheduleObservation::default(),
        at: Instant::now(),
        trigger: None,
        outcome: None,
        cause: None,
    });
}
