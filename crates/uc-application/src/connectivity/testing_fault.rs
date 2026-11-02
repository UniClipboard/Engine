//! 仅普通剪贴板验收作用域命中的真实获取中屏障。
use super::{ConnectionLease, ConnectivityError};
use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use serde::Serialize;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_observability_contract::wire_enum;
use uuid::Uuid;

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, Serialize)]
    pub enum AcquisitionEventKind {
        FlightRegistered => "flight_registered",
        BackendPolledPending => "backend_polled_pending",
        BackendPolledReady => "backend_polled_ready",
        Held => "held",
        Released => "released",
        HoldCancelled => "hold_cancelled",
        ReturnedSuccess => "returned_success",
        ReturnedFailure => "returned_failure",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AcquisitionEvent {
    pub seq: u64,
    pub at_ns: u64,
    pub attempt_alias: Uuid,
    pub hold_alias: Option<Uuid>,
    pub kind: AcquisitionEventKind,
}
#[derive(Debug, Clone, Serialize)]
pub struct AcquisitionHold {
    pub attempt_alias: Uuid,
    pub hold_alias: Uuid,
}
#[derive(Debug, Clone, Serialize)]
pub struct AcquisitionSnapshot {
    pub clock_alias: Uuid,
    pub now_ns: u64,
    pub remaining_hits: u32,
    pub holds: Vec<AcquisitionHold>,
    pub events: Vec<AcquisitionEvent>,
}
#[derive(Debug, thiserror::Error)]
pub enum AcquisitionControlError {
    #[error("acquisition hold plan is active")]
    Busy,
    #[error("acquisition hold capacity exceeded")]
    Capacity,
    #[error("acquisition observation overflow")]
    Overflow,
}
struct Hold {
    metadata: AcquisitionHold,
    token: CancellationToken,
}
struct State {
    clock_alias: Uuid,
    origin: Instant,
    remaining: u32,
    holds: Vec<Hold>,
    events: Vec<AcquisitionEvent>,
    overflow: bool,
}
impl State {
    fn record(&mut self, attempt: Uuid, hold: Option<Uuid>, kind: AcquisitionEventKind) {
        if self.events.len() >= 1024 {
            self.overflow = true;
            return;
        }
        self.events.push(AcquisitionEvent {
            seq: self.events.len() as u64 + 1,
            at_ns: self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            attempt_alias: attempt,
            hold_alias: hold,
            kind,
        });
    }
}
fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            Mutex::new(State {
                clock_alias: Uuid::new_v4(),
                origin: Instant::now(),
                remaining: 0,
                holds: Vec::new(),
                events: Vec::new(),
                overflow: false,
            })
        })
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub fn arm_acquisition_hold(hits: u32) -> Result<(), AcquisitionControlError> {
    let mut state = state();
    if state.overflow {
        return Err(AcquisitionControlError::Overflow);
    }
    if state.remaining != 0 {
        return Err(AcquisitionControlError::Busy);
    }
    if hits == 0 || hits > 64 || state.holds.len() + hits as usize > 64 {
        return Err(AcquisitionControlError::Capacity);
    }
    state.remaining = hits;
    Ok(())
}
pub fn acquisition_snapshot() -> Result<AcquisitionSnapshot, AcquisitionControlError> {
    let state = state();
    if state.overflow {
        return Err(AcquisitionControlError::Overflow);
    }
    Ok(AcquisitionSnapshot {
        clock_alias: state.clock_alias,
        now_ns: state.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        remaining_hits: state.remaining,
        holds: state
            .holds
            .iter()
            .map(|hold| hold.metadata.clone())
            .collect(),
        events: state.events.clone(),
    })
}
pub fn release_acquisition_hold(hold_alias: Uuid) -> bool {
    let mut state = state();
    if let Some(index) = state
        .holds
        .iter()
        .position(|hold| hold.metadata.hold_alias == hold_alias)
    {
        state.holds.swap_remove(index).token.cancel();
        true
    } else {
        false
    }
}
pub fn reset_acquisition_faults() {
    let mut state = state();
    state.remaining = 0;
    for hold in state.holds.drain(..) {
        hold.token.cancel();
    }
}
pub(super) struct Context {
    attempt: Uuid,
    observed: AtomicBool,
    started: CancellationToken,
}
tokio::task_local! { static CURRENT_ACQUISITION: Arc<Context>; }

/// Infra 只给普通剪贴板 connect 加作用域；不改变任何稳定连接 port。
pub async fn with_clipboard_acquisition(
    attempt_alias: Uuid,
    future: impl Future<Output = Result<ConnectionLease, ConnectivityError>>,
) -> Result<ConnectionLease, ConnectivityError> {
    let context = Arc::new(Context {
        attempt: attempt_alias,
        observed: AtomicBool::new(false),
        started: CancellationToken::new(),
    });
    let result = CURRENT_ACQUISITION.scope(context.clone(), future).await;
    if context.observed.load(Ordering::Acquire) {
        state().record(
            attempt_alias,
            None,
            if result.is_ok() {
                AcquisitionEventKind::ReturnedSuccess
            } else {
                AcquisitionEventKind::ReturnedFailure
            },
        );
    }
    result
}

pub(super) async fn after_registered(launched: bool, completed: impl Future<Output = ()>) {
    if !launched {
        return;
    }
    let Ok(context) = CURRENT_ACQUISITION.try_with(Arc::clone) else {
        return;
    };
    let wait = {
        let mut state = state();
        if state.remaining == 0 {
            return;
        }
        state.remaining -= 1;
        context.observed.store(true, Ordering::Release);
        let metadata = AcquisitionHold {
            attempt_alias: context.attempt,
            hold_alias: Uuid::new_v4(),
        };
        let token = CancellationToken::new();
        state.record(
            context.attempt,
            Some(metadata.hold_alias),
            AcquisitionEventKind::FlightRegistered,
        );
        state.holds.push(Hold {
            metadata: metadata.clone(),
            token: token.clone(),
        });
        Wait { metadata, token }
    };
    tokio::select! {
        biased;
        _ = wait.token.cancelled() => {},
        _ = context.started.cancelled() => {
            state().record(context.attempt, Some(wait.metadata.hold_alias), AcquisitionEventKind::Held);
            wait.token.cancelled().await;
        },
        _ = completed => return,
    }
    state().record(
        context.attempt,
        Some(wait.metadata.hold_alias),
        AcquisitionEventKind::Released,
    );
}
struct Wait {
    metadata: AcquisitionHold,
    token: CancellationToken,
}
impl Drop for Wait {
    fn drop(&mut self) {
        let mut state = state();
        if let Some(index) = state
            .holds
            .iter()
            .position(|hold| hold.metadata.hold_alias == self.metadata.hold_alias)
        {
            state.holds.swap_remove(index).token.cancel();
            state.record(
                self.metadata.attempt_alias,
                Some(self.metadata.hold_alias),
                AcquisitionEventKind::HoldCancelled,
            );
        }
    }
}

pub(super) fn capture_worker_context() -> Option<Arc<Context>> {
    CURRENT_ACQUISITION.try_with(Arc::clone).ok()
}

pub(super) async fn scope_worker<F: Future>(context: Option<Arc<Context>>, future: F) -> F::Output {
    if let Some(context) = context {
        CURRENT_ACQUISITION.scope(context, future).await
    } else {
        future.await
    }
}

pub(super) async fn poll_backend<F: Future>(future: F) -> F::Output {
    let context = CURRENT_ACQUISITION.try_with(Arc::clone).ok();
    tokio::pin!(future);
    poll_fn(|cx| {
        let result = future.as_mut().poll(cx);
        if let Some(context) = context.as_ref() {
            if !context.started.is_cancelled() {
                state().record(
                    context.attempt,
                    None,
                    if result.is_pending() {
                        AcquisitionEventKind::BackendPolledPending
                    } else {
                        AcquisitionEventKind::BackendPolledReady
                    },
                );
                context.started.cancel();
            }
        }
        result
    })
    .await
}
