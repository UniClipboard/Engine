use super::super::{
    ElapsedClockError, ElapsedClockPort, ProgressClaimRefusal, ProgressExchangeDeadline,
    ProgressLiveRoundPort, ProgressQueryResult, ProgressReceiveError, ProgressSnapshotRoundPort,
    ProgressSourceTask, ProgressSubmission, ProgressTaskAuthorityPort, ProgressTerminalRoundPort,
};
use super::clock::Clock;
use super::model::{clock_failure, Key, QueryKey};
use super::query::QUERY_LIMIT;
use super::state::State;
use super::worker::ProgressExchangeWorker;
use crate::connectivity::ConnectivityError;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    valid_progress_plan, ProgressActive, ProgressIdentity, ProgressSnapshotRefusal,
    ProgressTerminal, MAX_PROGRESS_ITEMS,
};
use uc_core::ids::DeviceId;
pub(super) struct Inner {
    pub state: Mutex<State>,
    pub clock: Arc<Clock>,
    pub wake: Arc<Notify>,
    pub authority: Arc<dyn ProgressTaskAuthorityPort>,
    pub round: Arc<dyn ProgressTerminalRoundPort>,
    pub snapshot: Arc<dyn ProgressSnapshotRoundPort>,
    pub live: Arc<dyn ProgressLiveRoundPort>,
}
/// 活动与结算通知的非网络等待提交能力及完整快照查询；不改变本地实际结果。
#[derive(Clone)]
pub struct ProgressExchangeService {
    pub(super) inner: Arc<Inner>,
}
impl ProgressExchangeService {
    pub fn new(
        clock: Arc<dyn ElapsedClockPort>,
        authority: Arc<dyn ProgressTaskAuthorityPort>,
        round: Arc<dyn ProgressTerminalRoundPort>,
        snapshot: Arc<dyn ProgressSnapshotRoundPort>,
        live: Arc<dyn ProgressLiveRoundPort>,
    ) -> (Self, ProgressExchangeWorker) {
        let wake = Arc::new(Notify::new());
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            clock: Arc::new(Clock::new(clock, wake.clone())),
            wake,
            authority,
            round,
            snapshot,
            live,
        });
        (
            Self {
                inner: inner.clone(),
            },
            ProgressExchangeWorker { inner },
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
                let source = Arc::new(source);
                let epoch = self.inner.clock.epoch();
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retire_before(epoch, Some(source.clone()));
                self.inner.wake.notify_one();
                return Err(ElapsedClockError {
                    source: Box::new(source),
                }
                .into());
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

/// 声明早于watch receiver，确保取消future先放下最后观察者，再唤醒worker清理。
struct WakeOnDrop(Arc<Notify>);
impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}
impl ProgressExchangeService {
    pub async fn read_snapshot(
        &self,
        peer: DeviceId,
        expected: ProgressSourceTask,
        cancel: &CancellationToken,
    ) -> ProgressQueryResult {
        if self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
        {
            return Arc::new(Err(ConnectivityError::Stopped.into()));
        }
        if !valid_progress_plan(&expected.items)
            || usize::from(expected.descriptor.item_count) != expected.items.len()
        {
            return Arc::new(Err(ProgressSnapshotRefusal::InvalidPlan.into()));
        }
        if cancel.is_cancelled() {
            return Arc::new(Err(ConnectivityError::Stopped.into()));
        }
        let _wake = WakeOnDrop(self.inner.wake.clone());
        let sample = match self.inner.clock.sample() {
            Ok(sample) => sample,
            Err(source) => {
                let source = Arc::new(source);
                let epoch = self.inner.clock.epoch();
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retire_before(epoch, Some(source.clone()));
                return clock_failure(Some(source));
            }
        };
        let mut receive = match self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .query(peer, expected, sample)
        {
            Ok(receive) => receive,
            Err(reply) => return reply,
        };
        self.inner.wake.notify_one();
        // 迟到的并发调用可能早于已入队请求采样；观察者也守住各自原五秒，不能借合并延长。
        let expires_at = match sample.now.checked_add(QUERY_LIMIT) {
            Some(deadline) => deadline,
            None => return Arc::new(Err(ConnectivityError::DeadlineExpired.into())),
        };
        let deadline =
            match ProgressExchangeDeadline::new(self.inner.clock.lease(sample.epoch), expires_at) {
                Ok(deadline) => deadline,
                Err(source) => return Arc::new(Err(source)),
            };
        loop {
            if let Err(source) = deadline.check(cancel) {
                return Arc::new(Err(source));
            }
            if let Some(reply) = receive.borrow_and_update().clone() {
                return reply;
            }
            tokio::select! {
                biased;
                _=cancel.cancelled()=>return Arc::new(Err(ConnectivityError::Stopped.into())),
                // discarded-source[channel]: watch关闭只表示唯一监督者退出，完整停止事实已明确。
                changed=receive.changed()=>if changed.is_err(){return Arc::new(Err(ConnectivityError::Stopped.into()));},
                _=tokio::time::sleep(Duration::from_millis(100))=>{},
            }
        }
    }
}

impl ProgressExchangeService {
    /// 只提交本地最新活动值；不等待网络或交换额度，终态封口后拒绝后续活动值。
    pub fn submit_live(
        &self,
        peer: DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        value: ProgressActive,
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
        let Some(task) = self.inner.authority.current_task(&peer, source) else {
            return Ok(ProgressSubmission::Refused(
                ProgressClaimRefusal::NotRetained,
            ));
        };
        if task.descriptor.source != source || task.descriptor.attempt != attempt {
            return Ok(ProgressSubmission::Refused(
                ProgressClaimRefusal::Superseded,
            ));
        }
        if task.items.is_empty()
            || task.items.len() > MAX_PROGRESS_ITEMS
            || usize::from(task.descriptor.item_count) != task.items.len()
        {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        if !task.items.contains(&value.item) {
            return Ok(ProgressSubmission::Refused(
                ProgressClaimRefusal::InvalidItem,
            ));
        }
        if task.notification_closed.contains(&value.item) {
            return Ok(ProgressSubmission::Refused(
                ProgressClaimRefusal::AlreadyObserved,
            ));
        }
        let sample = match self.inner.clock.sample() {
            Ok(sample) => sample,
            Err(source) => {
                let source = Arc::new(source);
                let epoch = self.inner.clock.epoch();
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retire_before(epoch, Some(source.clone()));
                self.inner.wake.notify_one();
                return Err(ElapsedClockError {
                    source: Box::new(source),
                }
                .into());
            }
        };
        let candidates = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .live_prune_candidates(peer, task.descriptor);
        // 只在元数据额度需要回收时核对已退休scope；不持状态锁调用权威能力。
        let obsolete: Vec<_> = candidates
            .into_iter()
            .filter(|(key, descriptor)| {
                self.inner
                    .authority
                    .current_task(&key.peer, descriptor.source)
                    .is_none_or(|current| QueryKey::new(key.peer, current.descriptor) != *key)
            })
            .map(|(key, _)| key)
            .collect();
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.prune_live_tasks(&obsolete);
        state.refresh(Some(sample));
        let submitted = state.admit_live(peer, task.descriptor, value, sample);
        self.inner.wake.notify_one();
        Ok(submitted)
    }
}
