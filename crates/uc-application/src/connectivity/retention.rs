//! 有界空闲保留与实际交换活动；入站 accept 等待不延长空闲期限。
use super::connection::ConnectionRecord;
use super::{
    ConnectionKey, ConnectivityError, ElapsedClockError, ElapsedClockPort, PeerConnection,
};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;
use uc_core::file_transfer::{BudgetStatus, ElapsedBudget};

#[derive(Clone)]
pub struct IdleRetentionPolicy {
    pub timeout: Duration,
    pub capacity: usize,
    /// 进度连接必须注入包含挂起的来源；其他协议显式保持原清醒计时。
    pub elapsed_clock: Option<Arc<dyn ElapsedClockPort>>,
}

struct Idle {
    since: Instant,
    budget: Option<ElapsedBudget>,
    failure: Option<Arc<ElapsedClockError>>,
}
#[derive(Default)]
struct ActivityState {
    users: usize,
    idle: Option<Idle>,
    retired: bool,
}
pub(super) struct ConnectionActivity {
    state: Mutex<ActivityState>,
    policy: Option<IdleRetentionPolicy>,
    pub changed: Notify,
}
pub(super) struct ActivityGuard(Arc<ConnectionActivity>);
impl ConnectionActivity {
    pub fn new(policy: Option<&IdleRetentionPolicy>) -> Self {
        Self {
            state: Mutex::new(ActivityState::default()),
            policy: policy.cloned(),
            changed: Notify::new(),
        }
    }
    pub fn sample_elapsed(&self) -> Result<Option<Duration>, ConnectivityError> {
        self.policy
            .as_ref()
            .and_then(|policy| policy.elapsed_clock.as_ref())
            .map(|clock| clock.elapsed())
            .transpose()
            .map_err(|source| ConnectivityError::Clock(Arc::new(source)))
    }
    fn start_idle(&self) -> Idle {
        let mut idle = Idle {
            since: Instant::now(),
            budget: None,
            failure: None,
        };
        if let Some(policy) = &self.policy {
            if let Some(clock) = &policy.elapsed_clock {
                let origin = match clock.elapsed() {
                    Ok(now) => Some(now),
                    Err(source) => {
                        idle.failure = Some(Arc::new(source));
                        None
                    }
                };
                idle.budget = Some(ElapsedBudget::start(origin, policy.timeout));
            }
        }
        idle
    }
    fn remaining(
        &self,
        state: &mut ActivityState,
    ) -> Result<Option<Duration>, Arc<ElapsedClockError>> {
        if state.users != 0 || self.policy.is_none() {
            return Ok(None);
        }
        let idle = state.idle.get_or_insert_with(|| self.start_idle());
        if let Some(source) = &idle.failure {
            return Err(source.clone());
        }
        let Some(policy) = &self.policy else {
            return Ok(None);
        };
        if let (Some(budget), Some(clock)) = (&mut idle.budget, &policy.elapsed_clock) {
            let now = match clock.elapsed() {
                Ok(now) => now,
                Err(source) => {
                    budget.apply(None);
                    let source = Arc::new(source);
                    idle.failure = Some(source.clone());
                    return Err(source);
                }
            };
            return Ok(Some(match budget.apply(Some(now)) {
                BudgetStatus::Remaining(left) => left,
                BudgetStatus::Expired => Duration::ZERO,
            }));
        }
        Ok(Some(policy.timeout.saturating_sub(idle.since.elapsed())))
    }
    pub fn acquire(
        self: &Arc<Self>,
        connection: &dyn PeerConnection,
    ) -> Result<Arc<ActivityGuard>, ConnectivityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.retired {
            return Err(ConnectivityError::Stopped);
        }
        let remaining = self.remaining(&mut state);
        if matches!(remaining, Ok(Some(left)) if left.is_zero()) || remaining.is_err() {
            state.retired = true;
            connection.close();
            return Err(match remaining {
                Err(source) => ConnectivityError::Clock(source),
                _ => ConnectivityError::DeadlineExpired,
            });
        }
        state.users += 1;
        state.idle = None;
        self.changed.notify_one();
        Ok(Arc::new(ActivityGuard(self.clone())))
    }
    pub fn idle_since(&self) -> Option<Instant> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.users == 0 {
            Some(state.idle.get_or_insert_with(|| self.start_idle()).since)
        } else {
            None
        }
    }
    pub fn idle_remaining(&self) -> Result<Option<Duration>, Arc<ElapsedClockError>> {
        self.remaining(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }
    /// 同一锁内核对活动与原截止，不能在检查后新取得活动再被旧计时关闭。
    pub fn expire_if_idle<K: ConnectionKey>(&self, record: &ConnectionRecord<K>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match self.remaining(&mut state) {
            Ok(None) => false,
            Ok(Some(left)) if !left.is_zero() => false,
            _ => {
                state.retired = true;
                record.connection.close();
                true
            }
        }
    }
    pub fn retire_if_idle<K: ConnectionKey>(
        &self,
        record: &ConnectionRecord<K>,
        before: Instant,
    ) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.users != 0 || state.idle.as_ref().is_none_or(|idle| idle.since > before) {
            return false;
        }
        state.retired = true;
        record.connection.close();
        true
    }
}
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.users -= 1;
        if state.users == 0 {
            state.idle = Some(self.0.start_idle());
        }
        self.0.changed.notify_one();
    }
}
