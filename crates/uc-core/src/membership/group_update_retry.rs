/// 组更新失败后的重试规则：提前尝试不能推迟已经安排的重试。
pub struct GroupUpdateRetryPolicy;

impl GroupUpdateRetryPolicy {
    /// 返回新的真实尝试次数与重试时刻；到期失败沿用三十秒至一小时的指数退避。
    pub fn after_failure(attempts: u32, scheduled_at_ms: i64, now_ms: i64) -> (u32, i64) {
        let attempts = attempts.saturating_add(1);
        let delay_ms = 30_000_i64
            .saturating_mul(1_i64 << attempts.saturating_sub(1).min(7))
            .min(3_600_000);
        let proposed_at_ms = now_ms.saturating_add(delay_ms);
        // 时钟回拨也不能让已有时刻超出本次退避上限；提前尝试只允许保留或提前。
        let retry_at_ms = if scheduled_at_ms > now_ms {
            scheduled_at_ms.min(proposed_at_ms)
        } else {
            proposed_at_ms
        };
        (attempts, retry_at_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::GroupUpdateRetryPolicy;

    #[test]
    fn early_attempts_keep_the_deadline_and_clock_discontinuities_remain_bounded() {
        assert_eq!(
            GroupUpdateRetryPolicy::after_failure(0, 0, 100),
            (1, 30_100)
        );
        assert_eq!(
            GroupUpdateRetryPolicy::after_failure(1, 30_100, 200),
            (2, 30_100)
        );
        assert_eq!(
            GroupUpdateRetryPolicy::after_failure(2, 30_100, 30_100),
            (3, 150_100)
        );
        assert_eq!(
            GroupUpdateRetryPolicy::after_failure(0, 4_000_000, 0),
            (1, 30_000)
        );
        assert_eq!(
            GroupUpdateRetryPolicy::after_failure(u32::MAX, 0, i64::MAX - 1),
            (u32::MAX, i64::MAX)
        );
    }
}
