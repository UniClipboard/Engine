//! 仅验收使用：在真实活跃状态准备完成后、最终门禁前暂停。
use super::{receive_snapshot, ReceiveAttempt, ReceiveObservationError, ReceiveStage};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct ConvergenceHold {
    pub hold_alias: Uuid,
    pub attempt_alias: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveHoldError {
    #[error(transparent)]
    Observation(#[from] ReceiveObservationError),
    #[error("hold hits must be between 1 and 64")]
    InvalidHits,
    #[error("an unconsumed convergence hold is already armed")]
    Busy,
}

#[derive(Default)]
struct Holds {
    remaining: u32,
    held: HashMap<Uuid, Held>,
}
struct Held {
    attempt: Uuid,
    release: CancellationToken,
    cancel: CancellationToken,
}
fn holds() -> MutexGuard<'static, Holds> {
    static HOLDS: OnceLock<Mutex<Holds>> = OnceLock::new();
    HOLDS
        .get_or_init(|| Mutex::new(Holds::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub fn arm_active_convergence_hold(hits: u32) -> Result<(), ReceiveHoldError> {
    receive_snapshot()?;
    if !(1..=64).contains(&hits) {
        return Err(ReceiveHoldError::InvalidHits);
    }
    let mut holds = holds();
    if holds.remaining != 0 {
        return Err(ReceiveHoldError::Busy);
    }
    holds.remaining = hits;
    Ok(())
}

pub fn release_active_convergence_hold(hold_alias: Uuid) -> bool {
    let holds = holds();
    let Some(held) = holds.held.get(&hold_alias) else {
        return false;
    };
    // 释放信号只接受一次；实际工作仍拥有登记，直到其退出时移除。
    if held.release.is_cancelled() || held.cancel.is_cancelled() {
        return false;
    }
    held.release.cancel();
    true
}

pub(super) fn reset() {
    let mut holds = holds();
    holds.remaining = 0;
    for held in holds.held.values() {
        held.cancel.cancel();
    }
}

pub(super) fn snapshot() -> (u32, Vec<ConvergenceHold>) {
    let holds = holds();
    (
        holds.remaining,
        holds
            .held
            .iter()
            .map(|(alias, held)| ConvergenceHold {
                hold_alias: *alias,
                attempt_alias: held.attempt,
            })
            .collect(),
    )
}

tokio::task_local! {
    pub(crate) static RECEIVE_SHUTDOWN: CancellationToken;
}

struct Wait {
    alias: Uuid,
    attempt: ReceiveAttempt,
    entry: String,
    finished: bool,
}
impl Drop for Wait {
    fn drop(&mut self) {
        holds().held.remove(&self.alias);
        if !self.finished {
            self.attempt
                .record(ReceiveStage::PreparationCancelled, Some(&self.entry));
        }
    }
}

pub(crate) async fn prepared(attempt: Option<ReceiveAttempt>, entry: &str) -> bool {
    let Some(attempt) = attempt else {
        return true;
    };
    attempt.record(ReceiveStage::Prepared, Some(entry));
    let release = CancellationToken::new();
    let cancel = CancellationToken::new();
    let alias = Uuid::new_v4();
    {
        let mut holds = holds();
        if holds.remaining == 0 {
            return true;
        }
        if holds.held.len() >= 64 {
            drop(holds);
            super::state().overflow = true;
            return false;
        }
        holds.remaining -= 1;
        holds.held.insert(
            alias,
            Held {
                attempt: attempt.alias,
                release: release.clone(),
                cancel: cancel.clone(),
            },
        );
    }
    let mut wait = Wait {
        alias,
        attempt,
        entry: entry.to_owned(),
        finished: false,
    };
    attempt.record(ReceiveStage::PreparationHeld, Some(entry));
    let shutdown = RECEIVE_SHUTDOWN.try_with(Clone::clone).unwrap_or_default();
    let (proceed, stage) = tokio::select! {
        biased;
        _ = shutdown.cancelled() => (false, ReceiveStage::PreparationCancelled),
        _ = cancel.cancelled() => (false, ReceiveStage::PreparationCancelled),
        _ = sleep(Duration::from_secs(30)) => (false, ReceiveStage::PreparationExpired),
        _ = release.cancelled() => (true, ReceiveStage::PreparationReleased),
    };
    attempt.record(stage, Some(entry));
    wait.finished = true;
    proceed
}
