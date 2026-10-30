//! 进程网络日志与 Engine 生命周期的交接。
//!
//! 网络日志属于进程，同一进程可能先后或同时存在多个 Engine。每个工作中（未暂停、未关闭）的 Engine 持有一份登记；
//! 最后一份登记结束时网络日志停车，交还运行目录的分段文件与租约，第一份登记开始时重新打开。宿主不直接停车：
//! 只有 Engine 生命周期负责人在发布暂停或关闭结果之前结束登记，在恢复之前开始登记。

use std::error::Error;
#[cfg(feature = "test-process-release")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use crate::local_file::lock;
use crate::runtime::{
    installed_network_log, installed_process_failure_blocks_registration,
    notify_engine_closer_changed,
};
use crate::SignalResult;

/// 工作中的 Engine 数；同时串行停车与重开，等待期间不让另一次转换交错。
static ACTIVE_ENGINES: Mutex<ActivityState> = Mutex::new(ActivityState {
    active: 0,
    abandoned: false,
    retained: Vec::new(),
    fence: None,
    parking: false,
});
static ACTIVITY_CHANGED: Condvar = Condvar::new();
static NETWORK_TRANSITION: Mutex<()> = Mutex::new(());
#[cfg(feature = "test-process-release")]
static FENCED_REGISTRATION_ATTEMPTS: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "test-process-release")]
pub(crate) fn fenced_registration_attempts() -> u64 {
    FENCED_REGISTRATION_ATTEMPTS.load(Ordering::SeqCst)
}

struct ActivityState {
    active: usize,
    abandoned: bool,
    retained: Vec<RetainedEngine>,
    fence: Option<Arc<()>>,
    parking: bool,
}

/// 进程持有的完整 Engine 关闭能力。
pub trait RetainedEngineCloser: Send + Sync {
    /// 等待已接受的工作结算，不重新执行关闭，也不确认资源释放。
    fn wait_accepted_work(&self);

    /// 只在进程显式重试时调用。
    fn retry_close(&self) -> RetainedEngineResult;

    /// 返回已完成关闭尝试的错误；不得在此启动新的关闭。
    fn errors(&self) -> Vec<Arc<dyn Error + Send + Sync>> {
        Vec::new()
    }
}

/// 完整 Engine 关闭能力的一次结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedEngineResult {
    Released,
    RetryableFailure,
    TerminalFailure,
}

/// 一个被进程持有的 Engine 关闭能力的身份。
#[derive(Clone)]
pub struct RetainedEngineToken(Arc<()>);

/// Engine 关闭工作与进程登记之间的结果状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedEngineState {
    Pending,
    Retryable,
    Terminal,
}

struct RetainedEngine {
    token: RetainedEngineToken,
    closer: Arc<dyn RetainedEngineCloser>,
    state: RetainedEngineState,
}

pub(crate) fn insert_engine_closer(
    closer: Arc<dyn RetainedEngineCloser>,
    state: RetainedEngineState,
) -> RetainedEngineToken {
    let token = RetainedEngineToken(Arc::new(()));
    lock(&ACTIVE_ENGINES).retained.push(RetainedEngine {
        token: token.clone(),
        closer,
        state,
    });
    ACTIVITY_CHANGED.notify_all();
    notify_engine_closer_changed();
    token
}

/// 更新同一个 Engine 已接受关闭工作的结果。
pub fn update_engine_closer(token: &RetainedEngineToken, outcome: RetainedEngineState) {
    let mut state = lock(&ACTIVE_ENGINES);
    if let Some(entry) = state
        .retained
        .iter_mut()
        .find(|entry| Arc::ptr_eq(&entry.token.0, &token.0))
    {
        entry.state = outcome;
    }
    drop(state);
    ACTIVITY_CHANGED.notify_all();
}

/// 确认同一个 Engine 已关闭后移除它的进程责任。
pub fn confirm_engine_closed(token: &RetainedEngineToken) {
    let removed = {
        let mut state = lock(&ACTIVE_ENGINES);
        state
            .retained
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.token.0, &token.0))
            .map(|index| state.retained.remove(index))
    };
    drop(removed);
    ACTIVITY_CHANGED.notify_all();
}

pub(crate) fn has_retryable_engine_closer() -> bool {
    lock(&ACTIVE_ENGINES)
        .retained
        .iter()
        .any(|entry| entry.state == RetainedEngineState::Retryable)
}

#[derive(Clone)]
pub(crate) struct RetainedEngineError {
    token: RetainedEngineToken,
    pub(crate) source: Arc<dyn Error + Send + Sync>,
}

impl RetainedEngineError {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.token.0, &other.token.0)
    }
}

pub(crate) fn retained_engine_errors() -> Vec<RetainedEngineError> {
    let closers = lock(&ACTIVE_ENGINES)
        .retained
        .iter()
        .map(|entry| (entry.token.clone(), Arc::clone(&entry.closer)))
        .collect::<Vec<_>>();
    closers
        .into_iter()
        .flat_map(|(token, closer)| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| closer.errors()))
                .unwrap_or_default()
                .into_iter()
                .map(move |source| RetainedEngineError {
                    token: token.clone(),
                    source,
                })
        })
        .collect()
}

pub(crate) fn has_unconfirmed_engine() -> bool {
    let state = lock(&ACTIVE_ENGINES);
    state.abandoned || !state.retained.is_empty()
}

#[cfg(feature = "test-process-release")]
pub(crate) fn retained_engine_count() -> usize {
    lock(&ACTIVE_ENGINES).retained.len()
}

pub(crate) fn retry_engine_closers() {
    let entries = {
        let state = lock(&ACTIVE_ENGINES);
        state
            .retained
            .iter()
            .filter(|entry| entry.state == RetainedEngineState::Retryable)
            .map(|entry| (entry.token.clone(), Arc::clone(&entry.closer)))
            .collect::<Vec<_>>()
    };
    for (token, closer) in entries {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| closer.retry_close()))
                .unwrap_or(RetainedEngineResult::TerminalFailure);
        match result {
            RetainedEngineResult::Released => confirm_engine_closed(&token),
            RetainedEngineResult::RetryableFailure => {}
            RetainedEngineResult::TerminalFailure => {
                update_engine_closer(&token, RetainedEngineState::Terminal)
            }
        }
    }
}

/// 接受或加入同一次登记封口；这本身不是资源释放回执。
pub(crate) fn fence_engines() -> Arc<()> {
    let mut state = lock(&ACTIVE_ENGINES);
    Arc::clone(state.fence.get_or_insert_with(|| Arc::new(())))
}

/// 等待被封口前的登记全部结束；旧封口不能证明新一代为空。
pub(crate) fn wait_fenced_idle(fence: &Arc<()>, deadline: Duration) -> bool {
    let started = Instant::now();
    let mut state = lock(&ACTIVE_ENGINES);
    loop {
        if !state
            .fence
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, fence))
        {
            return false;
        }
        let pending = state
            .retained
            .iter()
            .any(|entry| entry.state == RetainedEngineState::Pending);
        if !pending && (state.abandoned || !state.retained.is_empty()) {
            // 终止的关闭任务不代表其已接收的底层工作已经结算。
            // 仅进程工作线程使用无期限等待；零期限核对不得调用阻塞回调。
            if deadline == Duration::MAX && !state.retained.is_empty() {
                let retained = state
                    .retained
                    .iter()
                    .map(|entry| (entry.token.clone(), entry.state, Arc::clone(&entry.closer)))
                    .collect::<Vec<_>>();
                drop(state);
                for (_, _, closer) in &retained {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        closer.wait_accepted_work()
                    }));
                }
                state = lock(&ACTIVE_ENGINES);
                let unchanged = retained.len() == state.retained.len()
                    && retained.iter().all(|(token, status, _)| {
                        state.retained.iter().any(|entry| {
                            Arc::ptr_eq(&entry.token.0, &token.0) && entry.state == *status
                        })
                    });
                if !unchanged {
                    continue;
                }
            }
            return false;
        }
        if !pending && state.active == 0 && !state.parking {
            return true;
        }
        let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
            return false;
        };
        let (next, _) = ACTIVITY_CHANGED
            .wait_timeout(state, remaining.min(Duration::from_secs(60)))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state = next;
    }
}

/// 只有当前封口才能重新放行登记。
pub(crate) fn resume_engines(fence: &Arc<()>) -> bool {
    let mut state = lock(&ACTIVE_ENGINES);
    if !state
        .fence
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, fence))
    {
        return false;
    }
    state.fence = None;
    ACTIVITY_CHANGED.notify_all();
    true
}

/// 一个工作中 Engine 的登记。未确认结束时被丢弃会使进程释放保持未确认。
pub struct EngineLogActivity {
    ended: bool,
    admitted: bool,
}

impl EngineLogActivity {
    /// 登记一个开始工作的 Engine；从零变为一时重开停车中的网络日志。没有网络日志时返回 `None`。
    /// 无法重试的进程释放失败返回 `Failed`；登记超时返回 `TimedOut`。
    /// 会等待文件线程，不能在异步运行线程上调用。
    pub fn begin(deadline: Duration) -> (Self, Option<SignalResult>) {
        let started = Instant::now();
        loop {
            let Some(transition) = lock_transition(started, deadline) else {
                return not_admitted(SignalResult::TimedOut);
            };
            let mut state = lock(&ACTIVE_ENGINES);
            if let Some(fence) = state.fence.clone() {
                #[cfg(feature = "test-process-release")]
                FENCED_REGISTRATION_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                drop(state);
                drop(transition);
                let failed = installed_process_failure_blocks_registration(&fence);
                let state = lock(&ACTIVE_ENGINES);
                if !state
                    .fence
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &fence))
                {
                    continue;
                }
                if failed {
                    return not_admitted(SignalResult::Failed);
                }
                let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                    return not_admitted(SignalResult::TimedOut);
                };
                // 进程状态变化使用独立的条件变量；定期核对终止失败以避免漏唤醒。
                let (next, _) = ACTIVITY_CHANGED
                    .wait_timeout(state, remaining.min(Duration::from_millis(100)))
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drop(next);
                continue;
            }
            state.active = state.active.saturating_add(1);
            let first = state.active == 1;
            drop(state);
            let reopened = first
                .then(installed_network_log)
                .flatten()
                .map(|network| network.unpark(deadline.saturating_sub(started.elapsed())));
            return (
                Self {
                    ended: false,
                    admitted: true,
                },
                reopened,
            );
        }
    }

    /// 未通过进程登记门时为 false；不能把它当成有效登记。
    pub fn is_admitted(&self) -> bool {
        self.admitted
    }

    /// 结束登记；最后一份登记结束时停车，并等待已接受记录落盘、文件与租约交还。没有网络日志或仍有其他
    /// 工作中的 Engine 时返回 `None`。会等待文件线程，不能在异步运行线程上调用。
    pub fn end(&mut self, deadline: Duration) -> Option<SignalResult> {
        if !self.admitted {
            return None;
        }
        let started = Instant::now();
        let Some(_transition) = lock_transition(started, deadline) else {
            return Some(SignalResult::TimedOut);
        };
        let mut state = lock(&ACTIVE_ENGINES);
        if !self.ended {
            self.ended = true;
            state.active = state.active.saturating_sub(1);
        }
        let last = state.active == 0 && !state.abandoned && state.retained.is_empty();
        state.parking = last;
        drop(state);
        let result = last
            .then(installed_network_log)
            .flatten()
            .map(|network| network.park(deadline.saturating_sub(started.elapsed())));
        lock(&ACTIVE_ENGINES).parking = false;
        ACTIVITY_CHANGED.notify_all();
        result
    }
}

fn not_admitted(result: SignalResult) -> (EngineLogActivity, Option<SignalResult>) {
    (
        EngineLogActivity {
            ended: true,
            admitted: false,
        },
        Some(result),
    )
}

fn lock_transition(started: Instant, deadline: Duration) -> Option<MutexGuard<'static, ()>> {
    loop {
        match NETWORK_TRANSITION.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) if started.elapsed() >= deadline => return None,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

impl Drop for EngineLogActivity {
    fn drop(&mut self) {
        if !self.ended {
            let mut state = lock(&ACTIVE_ENGINES);
            state.active = state.active.saturating_sub(1);
            state.abandoned = true;
            ACTIVITY_CHANGED.notify_all();
            drop(state);
            notify_engine_closer_changed();
        }
    }
}
