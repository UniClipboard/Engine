//! 仅验收构建使用的真实内容流控制；不包含业务标识或负载。
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uc_application::deps::ReceiveCommitEvidence;
use uc_observability_contract::wire_enum;
use uuid::Uuid;

const EVENT_LIMIT: usize = 256;
const HOLD_LIMIT: usize = 64;

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
    pub enum FaultPhase {
        BeforeOpen => "BeforeOpen",
        BeforeFrame => "BeforeFrame",
        BeforeCompletion => "BeforeCompletion",
        ReceiverAppliedBeforeAck => "ReceiverAppliedBeforeAck",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
    pub enum FaultAction {
        Hold => "Hold",
        ResetSend => "ResetSend",
        ResetSendThenHoldCompletion => "ResetSendThenHoldCompletion",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, Serialize)]
    pub enum AttemptEventKind {
        Started => "Started",
        Reached => "Reached",
        Held => "Held",
        Released => "Released",
        HoldCancelled => "HoldCancelled",
        SendReset => "SendReset",
        ResultReadySuccess => "ResultReadySuccess",
        ResultReadyFailure => "ResultReadyFailure",
        ReturnedSuccess => "ReturnedSuccess",
        ReturnedFailure => "ReturnedFailure",
        Dropped => "Dropped",
        ReceiverApplied => "ReceiverApplied",
        ReceiverAckWriteStarted => "ReceiverAckWriteStarted",
        ReceiverAckAbandoned => "ReceiverAckAbandoned",
        ReceiverExchangeEnded => "ReceiverExchangeEnded",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AttemptEvent {
    pub attempt_alias: Uuid,
    pub phase: Option<FaultPhase>,
    pub kind: AttemptEventKind,
    pub elapsed_micros: u64,
    pub hold_alias: Option<Uuid>,
    pub receive_commit: Option<ReceiveCommitEvidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeldStream {
    pub hold_alias: Uuid,
    pub attempt_alias: Uuid,
    pub phase: FaultPhase,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaultSnapshot {
    pub remaining_hits: u32,
    pub hit_count: u64,
    pub held_attempts: Vec<Uuid>,
    pub holds: Vec<HeldStream>,
    pub pending_completion_holds: Vec<Uuid>,
    pub armed_phase: Option<FaultPhase>,
    pub armed_action: Option<FaultAction>,
    pub dropped_events: u64,
    pub events: Vec<AttemptEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum FaultControlError {
    #[error("unconsumed fault plan already active")]
    Busy,
    #[error("fault hit count must be between one and sixty-four")]
    InvalidHitCount,
    #[error("send reset requires the before-frame boundary")]
    InvalidPhase,
    #[error("fault hold capacity exhausted")]
    HoldCapacity,
}

struct Plan {
    phase: FaultPhase,
    action: FaultAction,
    remaining: u32,
}

struct Hold {
    metadata: HeldStream,
    token: CancellationToken,
}

struct State {
    origin: Instant,
    plan: Option<Plan>,
    hits: u64,
    held: HashMap<Uuid, Hold>,
    pending_completion: HashSet<Uuid>,
    dropped_events: u64,
    events: VecDeque<AttemptEvent>,
}

impl State {
    fn record(&mut self, alias: Uuid, phase: Option<FaultPhase>, kind: AttemptEventKind) {
        if self.events.len() == EVENT_LIMIT {
            self.events.pop_front();
            self.dropped_events += 1;
        }
        self.events.push_back(AttemptEvent {
            attempt_alias: alias,
            phase,
            kind,
            hold_alias: None,
            receive_commit: None,
            elapsed_micros: self.origin.elapsed().as_micros().min(u64::MAX as u128) as u64,
        });
    }

    fn record_hold(&mut self, metadata: &HeldStream, kind: AttemptEventKind) {
        self.record(metadata.attempt_alias, Some(metadata.phase), kind);
        if let Some(event) = self.events.back_mut() {
            event.hold_alias = Some(metadata.hold_alias);
        }
    }
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            Mutex::new(State {
                origin: Instant::now(),
                plan: None,
                hits: 0,
                held: HashMap::new(),
                pending_completion: HashSet::new(),
                dropped_events: 0,
                events: VecDeque::new(),
            })
        })
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 控制只属于当前进程；宿主须在独立验收进程中串行使用。
pub fn arm(phase: FaultPhase, action: FaultAction, hits: u32) -> Result<(), FaultControlError> {
    if hits == 0 || hits > 64 {
        return Err(FaultControlError::InvalidHitCount);
    }
    if matches!(
        action,
        FaultAction::ResetSend | FaultAction::ResetSendThenHoldCompletion
    ) && phase != FaultPhase::BeforeFrame
    {
        return Err(FaultControlError::InvalidPhase);
    }
    let mut state = state();
    if state.plan.is_some() {
        return Err(FaultControlError::Busy);
    }
    if matches!(
        action,
        FaultAction::Hold | FaultAction::ResetSendThenHoldCompletion
    ) && state.held.len() + state.pending_completion.len() + hits as usize > HOLD_LIMIT
    {
        return Err(FaultControlError::HoldCapacity);
    }
    state.plan = Some(Plan {
        phase,
        action,
        remaining: hits,
    });
    Ok(())
}

pub fn snapshot() -> FaultSnapshot {
    let state = state();
    FaultSnapshot {
        remaining_hits: state.plan.as_ref().map_or(0, |plan| plan.remaining),
        hit_count: state.hits,
        held_attempts: state
            .held
            .values()
            .map(|hold| hold.metadata.attempt_alias)
            .collect(),
        holds: state
            .held
            .values()
            .map(|hold| hold.metadata.clone())
            .collect(),
        pending_completion_holds: state.pending_completion.iter().copied().collect(),
        armed_phase: state.plan.as_ref().map(|plan| plan.phase),
        armed_action: state.plan.as_ref().map(|plan| plan.action),
        dropped_events: state.dropped_events,
        events: state.events.iter().cloned().collect(),
    }
}

/// 兼容单屏障宿主；新验收应使用 release_hold 按独立 token 释放。
pub fn release(attempt_alias: Uuid) -> bool {
    let mut state = state();
    let hold_alias = state
        .held
        .iter()
        .find_map(|(alias, hold)| (hold.metadata.attempt_alias == attempt_alias).then_some(*alias));
    if let Some(hold) = hold_alias.and_then(|alias| state.held.remove(&alias)) {
        hold.token.cancel();
        true
    } else {
        false
    }
}

/// 只释放这一个屏障；陈旧 token 不会影响同一尝试的后续屏障。
pub fn release_hold(hold_alias: Uuid) -> bool {
    let mut state = state();
    if let Some(hold) = state.held.remove(&hold_alias) {
        hold.token.cancel();
        true
    } else {
        false
    }
}

/// 解除计划和全部屏障，保留证据；不会重置别名或时间基准。
pub fn reset() {
    let mut state = state();
    state.plan = None;
    state.pending_completion.clear();
    for (_, hold) in state.held.drain() {
        hold.token.cancel();
    }
}

pub(super) struct Attempt {
    alias: Uuid,
    returned: bool,
}

impl Attempt {
    pub(super) fn alias(&self) -> Uuid {
        self.alias
    }

    pub(super) fn begin() -> Self {
        let alias = Uuid::new_v4();
        state().record(alias, None, AttemptEventKind::Started);
        Self {
            alias,
            returned: false,
        }
    }

    /// RESET 仍由实际发送方执行；组合动作只保留同一尝试的后续 Hold。
    pub(super) async fn reach(&self, phase: FaultPhase) -> Option<FaultAction> {
        let (action, wait) = {
            let mut state = state();
            state.record(self.alias, Some(phase), AttemptEventKind::Reached);
            let action = if phase == FaultPhase::BeforeCompletion
                && state.pending_completion.remove(&self.alias)
            {
                FaultAction::Hold
            } else {
                let plan = state.plan.as_mut().filter(|plan| plan.phase == phase)?;
                let action = plan.action;
                plan.remaining -= 1;
                if plan.remaining == 0 {
                    state.plan = None;
                }
                state.hits += 1;
                if action == FaultAction::ResetSendThenHoldCompletion {
                    state.pending_completion.insert(self.alias);
                }
                action
            };
            let wait = if action == FaultAction::Hold {
                let metadata = HeldStream {
                    hold_alias: Uuid::new_v4(),
                    attempt_alias: self.alias,
                    phase,
                };
                let token = CancellationToken::new();
                state.held.insert(
                    metadata.hold_alias,
                    Hold {
                        metadata: metadata.clone(),
                        token: token.clone(),
                    },
                );
                state.record_hold(&metadata, AttemptEventKind::Held);
                Some(HoldWait { metadata, token })
            } else {
                None
            };
            (action, wait)
        };
        if let Some(wait) = wait {
            wait.token.cancelled().await;
            state().record_hold(&wait.metadata, AttemptEventKind::Released);
        }
        Some(action)
    }

    pub(super) fn receiver_applied(commit: Option<ReceiveCommitEvidence>) -> Self {
        let alias = Uuid::new_v4();
        let mut state = state();
        state.record(
            alias,
            Some(FaultPhase::ReceiverAppliedBeforeAck),
            AttemptEventKind::ReceiverApplied,
        );
        if let Some(event) = state.events.back_mut() {
            event.receive_commit = commit;
        }
        Self {
            alias,
            returned: false,
        }
    }

    pub(super) fn receiver_ack_write(&self) {
        state().record(
            self.alias,
            Some(FaultPhase::ReceiverAppliedBeforeAck),
            AttemptEventKind::ReceiverAckWriteStarted,
        );
    }

    pub(super) fn receiver_end(&mut self, abandoned: bool) {
        self.returned = true;
        state().record(
            self.alias,
            Some(FaultPhase::ReceiverAppliedBeforeAck),
            if abandoned {
                AttemptEventKind::ReceiverAckAbandoned
            } else {
                AttemptEventKind::ReceiverExchangeEnded
            },
        );
    }

    pub(super) fn send_reset(&self) {
        state().record(
            self.alias,
            Some(FaultPhase::BeforeFrame),
            AttemptEventKind::SendReset,
        );
    }

    pub(super) fn result_ready(&self, success: bool) {
        state().record(
            self.alias,
            Some(FaultPhase::BeforeCompletion),
            if success {
                AttemptEventKind::ResultReadySuccess
            } else {
                AttemptEventKind::ResultReadyFailure
            },
        );
    }

    pub(super) fn returned(&mut self, success: bool) {
        self.returned = true;
        state().record(
            self.alias,
            None,
            if success {
                AttemptEventKind::ReturnedSuccess
            } else {
                AttemptEventKind::ReturnedFailure
            },
        );
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        let mut state = state();
        state.pending_completion.remove(&self.alias);
        // 等待 future 的 Drop 负责移除自己的 Hold；完整尝试不得遗留保留名额。
        if !self.returned {
            state.record(self.alias, None, AttemptEventKind::Dropped);
        }
    }
}

struct HoldWait {
    metadata: HeldStream,
    token: CancellationToken,
}

impl Drop for HoldWait {
    fn drop(&mut self) {
        let mut state = state();
        if let Some(hold) = state.held.remove(&self.metadata.hold_alias) {
            hold.token.cancel();
            state.record_hold(&self.metadata, AttemptEventKind::HoldCancelled);
        }
    }
}
