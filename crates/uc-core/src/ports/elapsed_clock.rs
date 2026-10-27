use std::error::Error;
use std::fmt;
use std::time::Duration;

use crate::error_class::ErrorClass;

/// 包含系统挂起时间的单调经过时间来源。
/// 读数不能换算成日期；读取失败保留原始来源，不借用墙钟延长期限。
pub trait ElapsedClockPort: Send + Sync {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError>;
}

#[derive(thiserror::Error)]
#[error("elapsed clock unavailable")]
pub struct ElapsedClockError {
    #[source]
    pub source: Box<dyn Error + Send + Sync>,
}
impl fmt::Debug for ElapsedClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
impl ErrorClass for ElapsedClockError {
    fn class(&self) -> &'static str {
        "elapsed_clock_unavailable"
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ElapsedClockRefusal {
    #[error("platform has no suspend-inclusive clock")]
    UnsupportedPlatform,
    #[error("native elapsed clock count is invalid")]
    InvalidNativeCount,
}

/// 期限预算的当前判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStatus {
    Remaining(Duration),
    Expired,
}

/// 固定绝对截止；任一次不可读、回退或到期后永久退休本预算。
///
/// 调用方传入读数，不在规则中读取时钟。恢复可用读数只能创建新意图的预算，
/// 不能复活旧预算；同时防止晚于起点、但早于上次观察的回退延长期限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElapsedBudget {
    deadline: Option<Duration>,
    observed: Option<Duration>,
}
impl ElapsedBudget {
    pub fn start(origin: Option<Duration>, total: Duration) -> Self {
        Self {
            deadline: origin.and_then(|origin| origin.checked_add(total)),
            observed: origin,
        }
    }

    pub fn apply(&mut self, now: Option<Duration>) -> BudgetStatus {
        if let (Some(deadline), Some(previous), Some(now)) = (self.deadline, self.observed, now) {
            if now >= previous {
                if let Some(left) = deadline.checked_sub(now).filter(|left| !left.is_zero()) {
                    self.observed = Some(now);
                    return BudgetStatus::Remaining(left);
                }
            }
        }
        self.deadline = None;
        BudgetStatus::Expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn remaining_shrinks_with_elapsed_time() {
        let mut budget = ElapsedBudget::start(Some(secs(100)), secs(600));
        assert_eq!(
            budget.apply(Some(secs(130))),
            BudgetStatus::Remaining(secs(570))
        );
    }

    #[test]
    fn suspend_jump_consumes_budget_without_resetting_it() {
        let mut budget = ElapsedBudget::start(Some(secs(10)), secs(600));
        // 挂起 400 秒后恢复：剩余按原期限计算，不是重新得到 600 秒。
        assert_eq!(
            budget.apply(Some(secs(410))),
            BudgetStatus::Remaining(secs(200))
        );
        // 恢复时已越过期限，直接过期而不是补发历史时刻。
        assert_eq!(budget.apply(Some(secs(10_000))), BudgetStatus::Expired);
    }

    #[test]
    fn exact_deadline_is_expired() {
        let mut budget = ElapsedBudget::start(Some(secs(0)), secs(5));
        assert_eq!(budget.apply(Some(secs(5))), BudgetStatus::Expired);
    }

    #[test]
    fn unreadable_or_regressing_clock_is_expired() {
        assert_eq!(
            ElapsedBudget::start(None, secs(5)).apply(Some(secs(1))),
            BudgetStatus::Expired
        );
        let mut budget = ElapsedBudget::start(Some(secs(50)), secs(600));
        assert_eq!(budget.apply(None), BudgetStatus::Expired);
        assert_eq!(budget.apply(Some(secs(49))), BudgetStatus::Expired);
    }

    #[test]
    fn unavailable_sample_permanently_retires_the_original_budget() {
        let mut budget = ElapsedBudget::start(Some(secs(100)), secs(600));
        assert_eq!(budget.apply(None), BudgetStatus::Expired);
        assert_eq!(budget.apply(Some(secs(101))), BudgetStatus::Expired);
    }

    #[test]
    fn every_observed_sample_must_be_nondecreasing() {
        let mut budget = ElapsedBudget::start(Some(secs(100)), secs(600));
        assert_eq!(
            budget.apply(Some(secs(500))),
            BudgetStatus::Remaining(secs(200))
        );
        assert_eq!(budget.apply(Some(secs(499))), BudgetStatus::Expired);
        assert_eq!(budget.apply(Some(secs(501))), BudgetStatus::Expired);
    }

    #[test]
    fn expiry_cannot_be_reversed_by_a_later_clock_reading() {
        let mut budget = ElapsedBudget::start(Some(secs(100)), secs(600));
        assert_eq!(budget.apply(Some(secs(700))), BudgetStatus::Expired);
        assert_eq!(budget.apply(Some(secs(699))), BudgetStatus::Expired);
    }
}
