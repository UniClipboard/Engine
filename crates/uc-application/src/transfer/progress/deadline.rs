//! 同一交换共享原绝对经过时间预算；网络每次poll先检查，实际本机工作由原负责人监督。
use super::{ElapsedClockPort, ProgressReceiveError};
use crate::connectivity::ConnectivityError;
use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;
use tokio::time::{interval_at, timeout_at, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{BudgetStatus, ElapsedBudget};

pub struct ProgressExchangeDeadline {
    clock: Arc<dyn ElapsedClockPort>,
    budget: Mutex<ElapsedBudget>,
    instant: Instant,
}
impl ProgressExchangeDeadline {
    pub fn new(
        clock: Arc<dyn ElapsedClockPort>,
        expires_at: Duration,
    ) -> Result<Self, ProgressReceiveError> {
        let started = Instant::now();
        let origin = clock.elapsed()?;
        let remaining = expires_at
            .checked_sub(origin)
            .filter(|left| !left.is_zero())
            .ok_or(ConnectivityError::DeadlineExpired)?;
        let instant = started
            .checked_add(remaining)
            .ok_or(ConnectivityError::DeadlineExpired)?;
        Ok(Self {
            clock,
            budget: Mutex::new(ElapsedBudget::start(Some(origin), remaining)),
            instant,
        })
    }
    /// 仅供既有受管连接等待使用；每次实际网络poll仍须检查经过时间预算。
    pub fn instant(&self) -> Instant {
        self.instant
    }
    pub fn check(&self, cancel: &CancellationToken) -> Result<(), ProgressReceiveError> {
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped.into());
        }
        let mut budget = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
        let now = match self.clock.elapsed() {
            Ok(now) => now,
            Err(source) => {
                budget.apply(None);
                return Err(source.into());
            }
        };
        if budget.apply(Some(now)) == BudgetStatus::Expired || Instant::now() >= self.instant {
            return Err(ConnectivityError::DeadlineExpired.into());
        }
        Ok(())
    }
    /// 只用于可安全中断的网络IO，不包装需要监督至实际结束的本机能力。
    pub async fn network<T>(
        &self,
        work: impl Future<Output = Result<T, ProgressReceiveError>>,
        cancel: &CancellationToken,
    ) -> Result<T, ProgressReceiveError> {
        let mut work = pin!(work);
        let guarded = poll_fn(|cx| {
            if let Err(error) = self.check(cancel) {
                return Poll::Ready(Err(error));
            }
            work.as_mut().poll(cx)
        });
        let mut guarded = pin!(guarded);
        // socket可能一直不唤醒；有界唤醒保证恢复后重新观察包含挂起的原预算。
        let pulse = Duration::from_millis(100);
        let mut recheck = interval_at(Instant::now() + pulse, pulse);
        recheck.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let result = loop {
            self.check(cancel)?;
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ConnectivityError::Stopped.into()),
                _ = recheck.tick() => continue,
                result = timeout_at(self.instant, guarded.as_mut()) => break result,
            }
        };
        // discarded-source[timeout]: 原交换截止已经表达全部超时事实。
        let result = result.map_err(|_| ConnectivityError::DeadlineExpired)??;
        self.check(cancel)?;
        Ok(result)
    }
}
