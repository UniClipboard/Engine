//! 仅验收使用的真实接收路径证据与按需拉取流 RESET。
mod pre_write;
pub use pre_write::{
    arm_active_convergence_hold, release_active_convergence_hold, ConvergenceHold, ReceiveHoldError,
};
pub(crate) use pre_write::{prepared, RECEIVE_SHUTDOWN};

use super::inbound::InboundClipboardApplyOutcome;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use serde::Serialize;
use tokio::time::Instant;
use uc_core::ids::DeviceId;
use uc_observability_contract::wire_enum;
use uuid::Uuid;

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub enum ReceivePath {
        ClipboardPush => "clipboard_push",
        ActivePull => "active_pull",
        ActiveConvergence => "active_convergence",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub enum ReceiveStage {
        Started => "started",
        StreamOpened => "stream_opened",
        SendReset => "send_reset",
        EnvelopeReceived => "envelope_received",
        ApplyStarted => "apply_started",
        Applied => "applied",
        Duplicate => "duplicate",
        Failed => "failed",
        Prepared => "prepared",
        PreparationHeld => "preparation_held",
        PreparationReleased => "preparation_released",
        PreparationCancelled => "preparation_cancelled",
        PreparationExpired => "preparation_expired",
        GlobalGateRejected => "global_gate_rejected",
        WriteHandoff => "write_handoff",
        OsWriteStarted => "os_write_started",
        OsWriteSucceeded => "os_write_succeeded",
        OsWriteFailed => "os_write_failed",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiveEvent {
    pub seq: u64,
    pub at_ns: u64,
    pub attempt_alias: Uuid,
    pub item_alias: Uuid,
    pub entry_alias: Option<Uuid>,
    pub path: ReceivePath,
    pub stage: ReceiveStage,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiveSnapshot {
    pub epoch_alias: Uuid,
    pub now_ns: u64,
    pub reset_active_pull: bool,
    pub convergence_remaining_hits: u32,
    pub convergence_holds: Vec<ConvergenceHold>,
    pub os_write_calls: u64,
    pub events: Vec<ReceiveEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveObservationError {
    #[error("receive observation has not started")]
    Disabled,
    #[error("receive observation overflow invalidated this epoch")]
    Overflow,
    #[error("previous receive work crossed the observation epoch")]
    EpochChanged,
}

struct State {
    epoch: Uuid,
    origin: Instant,
    enabled: bool,
    reset_pull: bool,
    os_write_calls: u64,
    overflow: bool,
    epoch_changed: bool,
    items: HashMap<(DeviceId, String), Uuid>,
    entries: HashMap<String, Uuid>,
    events: Vec<ReceiveEvent>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            epoch: Uuid::new_v4(),
            origin: Instant::now(),
            enabled: false,
            reset_pull: false,
            os_write_calls: 0,
            overflow: false,
            epoch_changed: false,
            items: HashMap::new(),
            entries: HashMap::new(),
            events: Vec::new(),
        }
    }
}
impl State {
    fn time(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }
}
fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 新空闲场景中开始观察；true 只 RESET 实际按需拉取请求流。
pub fn begin_receive_observation(reset_active_pull: bool) -> Uuid {
    pre_write::reset();
    let mut state = state();
    *state = State {
        enabled: true,
        reset_pull: reset_active_pull,
        ..State::default()
    };
    state.epoch
}

/// 关闭故障，不清除原证据；宿主应在关闭 Engine 前调用。
pub fn reset_receive_faults() {
    pre_write::reset();
    state().reset_pull = false;
}

pub fn receive_snapshot() -> Result<ReceiveSnapshot, ReceiveObservationError> {
    let state = state();
    if !state.enabled {
        return Err(ReceiveObservationError::Disabled);
    }
    if state.overflow {
        return Err(ReceiveObservationError::Overflow);
    }
    if state.epoch_changed {
        return Err(ReceiveObservationError::EpochChanged);
    }
    let (convergence_remaining_hits, convergence_holds) = pre_write::snapshot();
    Ok(ReceiveSnapshot {
        epoch_alias: state.epoch,
        now_ns: state.time(),
        reset_active_pull: state.reset_pull,
        convergence_remaining_hits,
        convergence_holds,
        os_write_calls: state.os_write_calls,
        events: state.events.clone(),
    })
}

#[derive(Clone, Copy)]
pub(crate) struct ReceiveAttempt {
    epoch: Uuid,
    alias: Uuid,
    item: Uuid,
    path: ReceivePath,
}
impl ReceiveAttempt {
    pub(crate) fn begin(path: ReceivePath, peer: DeviceId, hash: &str) -> Option<Self> {
        let mut state = state();
        if !state.enabled || state.overflow {
            return None;
        }
        let key = (peer, hash.to_owned());
        if !state.items.contains_key(&key) && state.items.len() >= 256 {
            state.overflow = true;
            return None;
        }
        let item = *state.items.entry(key).or_insert_with(Uuid::new_v4);
        let attempt = Self {
            epoch: state.epoch,
            alias: Uuid::new_v4(),
            item,
            path,
        };
        drop(state);
        attempt.record(ReceiveStage::Started, None);
        Some(attempt)
    }
    pub(crate) fn record(self, stage: ReceiveStage, entry: Option<&str>) {
        let mut state = state();
        if state.epoch != self.epoch {
            state.epoch_changed = true;
            return;
        }
        if state.events.len() >= 1024 {
            state.overflow = true;
            return;
        }
        let entry_alias = if let Some(entry) = entry {
            if !state.entries.contains_key(entry) && state.entries.len() >= 256 {
                state.overflow = true;
                return;
            }
            Some(
                *state
                    .entries
                    .entry(entry.to_owned())
                    .or_insert_with(Uuid::new_v4),
            )
        } else {
            None
        };
        let event = ReceiveEvent {
            seq: state.events.len() as u64 + 1,
            at_ns: state.time(),
            attempt_alias: self.alias,
            item_alias: self.item,
            entry_alias,
            path: self.path,
            stage,
        };
        state.events.push(event);
    }

    pub(crate) fn apply_outcome(self, outcome: &InboundClipboardApplyOutcome) {
        let (stage, entry) = match outcome {
            InboundClipboardApplyOutcome::Applied { entry_id } => {
                (ReceiveStage::Applied, Some(entry_id.as_str()))
            }
            InboundClipboardApplyOutcome::Resurfaced {
                existing_entry_id, ..
            }
            | InboundClipboardApplyOutcome::DuplicateSkipped {
                existing_entry_id, ..
            } => (ReceiveStage::Duplicate, Some(existing_entry_id.as_str())),
            InboundClipboardApplyOutcome::DecodeFailed { .. } => (ReceiveStage::Failed, None),
        };
        self.record(stage, entry);
    }
}

tokio::task_local! {
    pub(crate) static CURRENT_RECEIVE: Option<ReceiveAttempt>;
}

pub(crate) fn current_stage(stage: ReceiveStage) {
    if let Ok(Some(attempt)) = CURRENT_RECEIVE.try_with(|attempt| *attempt) {
        attempt.record(stage, None);
    }
}

pub(crate) fn reset_active_pull() -> bool {
    let mut state = state();
    let reset = state.enabled && state.reset_pull;
    if reset
        && !CURRENT_RECEIVE
            .try_with(|attempt| attempt.is_some_and(|attempt| attempt.epoch == state.epoch))
            .unwrap_or(false)
    {
        state.epoch_changed = true;
    }
    reset
}

pub(crate) fn current_apply_outcome(outcome: &InboundClipboardApplyOutcome) {
    if let Ok(Some(attempt)) = CURRENT_RECEIVE.try_with(|attempt| *attempt) {
        attempt.apply_outcome(outcome);
    }
}

#[derive(Clone, Copy)]
pub struct ReceiveCommitCursor {
    epoch: Uuid,
    next_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiveCommitEvidence {
    pub epoch_alias: Uuid,
    pub applied_seq: u64,
    pub receive_attempt_alias: Uuid,
    pub item_alias: Uuid,
    pub entry_alias: Uuid,
    pub path: ReceivePath,
}

/// 在实际广播之前取得游标，禁止用更早的保存结果归因当前回执。
pub fn receive_commit_cursor() -> Option<ReceiveCommitCursor> {
    let state = state();
    state.enabled.then_some(ReceiveCommitCursor {
        epoch: state.epoch,
        next_seq: state.events.len() as u64 + 1,
    })
}

/// 只查找本次广播之后、同认证来源和内容身份的真实新条目保存结果。
pub fn applied_push_since(
    cursor: Option<ReceiveCommitCursor>,
    peer: DeviceId,
    hash: &str,
) -> Option<ReceiveCommitEvidence> {
    let cursor = cursor?;
    let mut state = state();
    if state.epoch != cursor.epoch {
        state.epoch_changed = true;
        return None;
    }
    if state.overflow {
        return None;
    }
    let item = *state.items.get(&(peer, hash.to_owned()))?;
    let event = state.events.iter().rev().find(|event| {
        event.seq >= cursor.next_seq
            && event.item_alias == item
            && event.path == ReceivePath::ClipboardPush
            && event.stage == ReceiveStage::Applied
    })?;
    Some(ReceiveCommitEvidence {
        epoch_alias: state.epoch,
        applied_seq: event.seq,
        receive_attempt_alias: event.attempt_alias,
        item_alias: item,
        entry_alias: event.entry_alias?,
        path: event.path,
    })
}

/// 显式传递同一次尝试，跨越写入负责人任务和阻塞系统调用。
pub(crate) fn current_attempt() -> Option<ReceiveAttempt> {
    CURRENT_RECEIVE.try_with(|attempt| *attempt).ok().flatten()
}

pub(crate) fn os_write_started(attempt: Option<ReceiveAttempt>) {
    {
        let mut state = state();
        if state.enabled {
            state.os_write_calls = state.os_write_calls.saturating_add(1);
        }
    }
    if let Some(attempt) = attempt {
        attempt.record(ReceiveStage::OsWriteStarted, None);
    }
}
