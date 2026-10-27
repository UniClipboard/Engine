use std::time::Duration;

/// 包含系统挂起时间的单调经过时间来源。
///
/// 返回值是相对同一进程内固定但不透明起点的读数，同一实现的读数不得因系统挂起而停走。
/// 它不是墙钟，不能换算为日期；读不到或平台不能保证包含挂起时返回 `None`。
pub trait ElapsedClockPort: Send + Sync {
    fn elapsed(&self) -> Option<Duration>;
}

/// 期限预算的当前判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStatus {
    /// 预算尚有剩余。
    Remaining(Duration),
    /// 预算已耗尽，或因读数不可信按耗尽处理。
    Expired,
}

/// 从固定起点开始、不可重置的经过时间预算。
///
/// 判定只依赖调用方传入的两次读数：起点读数不可用、当前读数不可用或读数倒退时，
/// 无法证明预算仍有剩余，一律保守按耗尽处理，调用方据此显示未确认或停止补发，
/// 不得用其他时钟延长期限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElapsedBudget {
    origin: Option<Duration>,
    total: Duration,
}

impl ElapsedBudget {
    pub fn start(origin: Option<Duration>, total: Duration) -> Self {
        Self { origin, total }
    }

    pub fn status(&self, now: Option<Duration>) -> BudgetStatus {
        let (Some(origin), Some(now)) = (self.origin, now) else {
            return BudgetStatus::Expired;
        };
        let Some(spent) = now.checked_sub(origin) else {
            return BudgetStatus::Expired;
        };
        match self.total.checked_sub(spent) {
            Some(left) if !left.is_zero() => BudgetStatus::Remaining(left),
            _ => BudgetStatus::Expired,
        }
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
        let budget = ElapsedBudget::start(Some(secs(100)), secs(600));
        assert_eq!(
            budget.status(Some(secs(130))),
            BudgetStatus::Remaining(secs(570))
        );
    }

    #[test]
    fn suspend_jump_consumes_budget_without_resetting_it() {
        let budget = ElapsedBudget::start(Some(secs(10)), secs(600));
        // 挂起 400 秒后恢复：剩余按原期限计算，不是重新得到 600 秒。
        assert_eq!(
            budget.status(Some(secs(410))),
            BudgetStatus::Remaining(secs(200))
        );
        // 恢复时已越过期限，直接过期而不是补发历史时刻。
        assert_eq!(budget.status(Some(secs(10_000))), BudgetStatus::Expired);
    }

    #[test]
    fn exact_deadline_is_expired() {
        let budget = ElapsedBudget::start(Some(secs(0)), secs(5));
        assert_eq!(budget.status(Some(secs(5))), BudgetStatus::Expired);
    }

    #[test]
    fn unreadable_or_regressing_clock_is_expired() {
        assert_eq!(
            ElapsedBudget::start(None, secs(5)).status(Some(secs(1))),
            BudgetStatus::Expired
        );
        let budget = ElapsedBudget::start(Some(secs(50)), secs(600));
        assert_eq!(budget.status(None), BudgetStatus::Expired);
        assert_eq!(budget.status(Some(secs(49))), BudgetStatus::Expired);
    }
}
