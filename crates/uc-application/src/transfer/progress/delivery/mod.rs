//! 原结算只领取一次通知意图；保留、补发、跨端调度及实际工作结算由本模块监督。
mod clock;
mod state;

use self::clock::Clock;
use self::state::{Key, State};
use super::{
    ElapsedClockPort, ProgressExchangeDeadline, ProgressReceiveError, ProgressSubmission,
    ProgressTaskAuthorityPort, ProgressTerminalRoundPort,
};
use crate::connectivity::ConnectivityError;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{ProgressIdentity, ProgressTerminal};
use uc_core::ids::DeviceId;
use uc_observability_contract::uc_debug;

struct Inner {
    state: Mutex<State>,
    clock: Arc<Clock>,
    wake: Arc<Notify>,
    authority: Arc<dyn ProgressTaskAuthorityPort>,
    round: Arc<dyn ProgressTerminalRoundPort>,
}
/// 完整结算后的非网络等待提交能力；不持久化队列，不改变本地实际结果。
#[derive(Clone)]
pub struct ProgressTerminalDelivery {
    inner: Arc<Inner>,
}
/// 唯一拥有全部单轮future的生命周期句柄；取消后仍须等待run返回。
pub struct ProgressDeliveryWorker {
    inner: Arc<Inner>,
}
impl ProgressTerminalDelivery {
    pub fn new(
        clock: Arc<dyn ElapsedClockPort>,
        authority: Arc<dyn ProgressTaskAuthorityPort>,
        round: Arc<dyn ProgressTerminalRoundPort>,
    ) -> (Self, ProgressDeliveryWorker) {
        let wake = Arc::new(Notify::new());
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            clock: Arc::new(Clock::new(clock, wake.clone())),
            wake,
            authority,
            round,
        });
        (
            Self {
                inner: inner.clone(),
            },
            ProgressDeliveryWorker { inner },
        )
    }
    pub fn submit_settled(
        &self,
        peer: DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        result: ProgressTerminal,
    ) -> Result<ProgressSubmission, ProgressReceiveError> {
        if self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
        {
            return Ok(ProgressSubmission::Closed);
        }
        // 权威只做本机有界原子领取；不持状态锁调用能力，也不在此等待网络。
        let current = match self
            .inner
            .authority
            .claim_terminal(&peer, source, attempt, result.item)
        {
            Ok(current) => current,
            Err(reason) => return Ok(ProgressSubmission::Refused(reason)),
        };
        let sample = match self.inner.clock.sample() {
            Ok(sample) => sample,
            Err(source) => {
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retire_before(self.inner.clock.epoch());
                self.inner.wake.notify_one();
                return Err(source.into());
            }
        };
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Ok(ProgressSubmission::Closed);
        }
        state.refresh(Some(sample));
        let retained = state.admit(
            Key::new(peer, current.descriptor, result),
            current.descriptor,
            result,
            sample,
        );
        self.inner.wake.notify_one();
        Ok(if retained {
            ProgressSubmission::Retained
        } else {
            ProgressSubmission::NotRetained
        })
    }
    /// 机会只消费原剩余时点；重复恢复通知合并，不重置预算。
    pub fn network_recovered(&self, peer: Option<DeviceId>) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recovered(peer);
        self.inner.wake.notify_one();
    }
}
impl ProgressDeliveryWorker {
    pub async fn run(self, cancel: CancellationToken) {
        let mut running: FuturesUnordered<
            BoxFuture<'static, (Key, Result<(), ProgressReceiveError>)>,
        > = FuturesUnordered::new();
        let mut clock_unavailable = false;
        loop {
            let sample = self.inner.clock.sample();
            if let Err(source) = &sample {
                if !clock_unavailable {
                    uc_debug!(
                        error_class = source.class(),
                        error = source as &dyn Error,
                        "progress notification clock unavailable"
                    );
                }
            }
            clock_unavailable = sample.is_err();
            let jobs = {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if cancel.is_cancelled() {
                    state.close();
                }
                match sample {
                    Ok(sample) if !state.closed => state.dispatch(sample),
                    _ => {
                        state.retire_before(self.inner.clock.epoch());
                        Vec::new()
                    }
                }
            };
            for job in jobs {
                let clock = self.inner.clock.lease(job.epoch);
                let round = self.inner.round.clone();
                running.push(Box::pin(async move {
                    let result = match ProgressExchangeDeadline::new(clock, job.deadline) {
                        Ok(deadline) => {
                            round
                                .report_settled(
                                    &job.key.peer,
                                    job.descriptor,
                                    job.result,
                                    &deadline,
                                    &job.cancel,
                                )
                                .await
                        }
                        Err(source) => Err(source),
                    };
                    (job.key, result)
                }));
            }
            let (closed, has_work) = {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                (state.closed, state.has_work())
            };
            if closed && running.is_empty() {
                break;
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !closed => {},
                Some((key, result)) = running.next(), if !running.is_empty() => {
                    let class = match &result { Ok(()) => RetryClass::Stop, Err(source) => retry_class(source) };
                    if let Err(source) = &result {
                        if !closed {
                            uc_debug!(error_class = source.class(), error = source as &dyn Error, "progress terminal notification round ended without acknowledgment");
                        }
                    }
                    self.inner.state.lock().unwrap_or_else(PoisonError::into_inner).finish(key, result.is_ok(), matches!(class, RetryClass::Retry), matches!(class, RetryClass::Cancelled));
                    tokio::task::yield_now().await;
                },
                _ = self.inner.wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_millis(100)), if has_work => {},
            }
        }
    }
}
impl Drop for ProgressDeliveryWorker {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close();
    }
}

enum RetryClass {
    Retry,
    Cancelled,
    Stop,
}
fn retry_class(source: &ProgressReceiveError) -> RetryClass {
    match source {
        ProgressReceiveError::Connection(source) => connection_retry(source),
        ProgressReceiveError::Exchange(_) => RetryClass::Retry,
        ProgressReceiveError::Clock(_)
        | ProgressReceiveError::Unsupported
        | ProgressReceiveError::Protocol
        | ProgressReceiveError::UnknownTask
        | ProgressReceiveError::InvalidPlan
        | ProgressReceiveError::Stale
        | ProgressReceiveError::Conflict => RetryClass::Stop,
    }
}
fn connection_retry(source: &ConnectivityError) -> RetryClass {
    match source {
        ConnectivityError::AddressMissing
        | ConnectivityError::Offline
        | ConnectivityError::Busy(_)
        | ConnectivityError::DeadlineExpired
        | ConnectivityError::TimedOut(_)
        | ConnectivityError::AdmissionDeadlineExpired
        | ConnectivityError::AdmissionTimedOut(_)
        | ConnectivityError::AddressLookup(_)
        | ConnectivityError::Backend(_)
        | ConnectivityError::Admission(_)
        | ConnectivityError::Stream(_) => RetryClass::Retry,
        ConnectivityError::Stopped => RetryClass::Cancelled,
        ConnectivityError::Shared(source) => connection_retry(source),
        ConnectivityError::ResourceClosed(_)
        | ConnectivityError::ResultClosed(_)
        | ConnectivityError::InboundResultClosed(_)
        | ConnectivityError::Task(_)
        | ConnectivityError::ExchangeAlreadyUsed
        | ConnectivityError::Capability(_)
        | ConnectivityError::Denied
        | ConnectivityError::Settlement { .. } => RetryClass::Stop,
    }
}
