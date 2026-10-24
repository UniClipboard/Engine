//! 单个逻辑接收项的时间预算；自动重连保留同一实例，不按网络轮次重建。
use crate::transfer::policy::{
    DATA_STALL_TIMEOUT, FETCH_BACKOFFS, FIRST_DATA_TIMEOUT, MAX_FETCH_ATTEMPTS, RECOVERY_TIMEOUT,
};
use std::time::Duration;
use tokio::time::Instant;
use uc_core::error_class::ErrorClass;
use uc_observability_contract::wire_enum;

wire_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    pub(in super::super) enum TimingRefusal: &'static str {
        #[error("blob first data deadline expired")]
        FirstDataExpired => "blob_first_data_expired",
        #[error("blob validated data stalled")]
        DataStalled => "blob_data_stalled",
        #[error("blob cumulative recovery deadline expired")]
        RecoveryExpired => "blob_recovery_expired",
        #[error("blob fetch attempts exhausted")]
        AttemptsExhausted => "blob_attempts_exhausted",
        #[error("blob retry backoff has not elapsed")]
        RetryNotReady => "blob_retry_not_ready",
        #[error("blob retry was not scheduled")]
        RetryNotScheduled => "blob_retry_not_scheduled",
    }
}

#[derive(Debug, thiserror::Error)]
#[error("blob receive budget refused operation")]
pub(in super::super) struct TimingError(#[source] pub TimingRefusal);

impl ErrorClass for TimingError {
    fn class(&self) -> &'static str {
        self.0.wire()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in super::super) struct ReceiveDeadline {
    pub at: Instant,
    pub reason: TimingRefusal,
}

pub(in super::super) struct ReceiveTiming {
    activated_at: Instant,
    last_data_at: Option<Instant>,
    verified_end: u64,
    attempts_started: u32,
    recovery_used: Duration,
    recovering_since: Option<Instant>,
    retry_not_before: Option<Instant>,
}

impl ReceiveTiming {
    pub fn new(activated_at: Instant) -> Self {
        Self {
            activated_at,
            last_data_at: None,
            verified_end: 0,
            attempts_started: 1,
            recovery_used: Duration::ZERO,
            recovering_since: None,
            retry_not_before: None,
        }
    }

    pub fn deadline(&self) -> ReceiveDeadline {
        let data_deadline = match self.last_data_at {
            Some(last_data_at) => ReceiveDeadline {
                at: last_data_at + DATA_STALL_TIMEOUT,
                reason: TimingRefusal::DataStalled,
            },
            None => ReceiveDeadline {
                at: self.activated_at + FIRST_DATA_TIMEOUT,
                reason: TimingRefusal::FirstDataExpired,
            },
        };
        if let Some(since) = self.recovering_since {
            let recovery_at = since + RECOVERY_TIMEOUT.saturating_sub(self.recovery_used);
            if recovery_at < data_deadline.at {
                return ReceiveDeadline {
                    at: recovery_at,
                    reason: TimingRefusal::RecoveryExpired,
                };
            }
        }
        data_deadline
    }

    /// 在输出合法空 blob／网络正常结束之前也核对期限；不伪造数据回调。
    pub fn check(&self, now: Instant) -> Result<(), TimingError> {
        let deadline = self.deadline();
        if now >= deadline.at {
            return Err(TimingError(deadline.reason));
        }
        Ok(())
    }

    /// 只接受 SDK 已验证、按 Raw 内容顺序推进的末尾位置。重传／重复事件不续期。
    pub fn validated_through(&mut self, end: u64, now: Instant) -> Result<bool, TimingError> {
        self.check(now)?;
        if end <= self.verified_end {
            return Ok(false);
        }
        self.verified_end = end;
        self.last_data_at = Some(now);
        if let Some(since) = self.recovering_since.take() {
            self.recovery_used += now.saturating_duration_since(since);
        }
        Ok(true)
    }

    /// 仅由完整负责人对来源白名单中的暂态网络错误调用；不自行推断宽泛错误种类。
    pub fn schedule_recovery(&mut self, now: Instant) -> Result<Instant, TimingError> {
        self.check(now)?;
        if let Some(not_before) = self.retry_not_before {
            return Ok(not_before);
        }
        if self.attempts_started >= MAX_FETCH_ATTEMPTS {
            return Err(TimingError(TimingRefusal::AttemptsExhausted));
        }
        let backoff = FETCH_BACKOFFS
            .get((self.attempts_started - 1) as usize)
            .copied()
            .ok_or(TimingError(TimingRefusal::AttemptsExhausted))?;
        self.recovering_since.get_or_insert(now);
        let not_before = now + backoff;
        self.retry_not_before = Some(not_before);
        Ok(not_before)
    }

    /// 等待退避与截止时间由 transport 同时监督；计数只在实际开始新一轮时增加。
    pub fn start_retry(&mut self, now: Instant) -> Result<(), TimingError> {
        self.check(now)?;
        let not_before = self
            .retry_not_before
            .ok_or(TimingError(TimingRefusal::RetryNotScheduled))?;
        if now < not_before {
            return Err(TimingError(TimingRefusal::RetryNotReady));
        }
        self.retry_not_before = None;
        self.attempts_started += 1;
        Ok(())
    }
}

#[cfg(test)]
#[path = "timing_tests.rs"]
mod tests;
