use super::super::clock::Clock;
use super::super::{
    ElapsedClockError, ElapsedClockPort, ProgressClaimRefusal, ProgressExchangeDeadline,
    ProgressLiveRoundPort, ProgressQueryResult, ProgressReceiveError, ProgressSnapshotRoundPort,
    ProgressSnapshotTarget, ProgressSubmission, ProgressTaskAuthorityPort, ProgressTaskDescriptor,
    ProgressTerminalRoundPort,
};
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
        expected: ProgressTaskDescriptor,
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
            .claim_terminal(&peer, expected, result.item)
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
        target: ProgressSnapshotTarget,
        intent: ProgressIdentity,
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
        if !valid_progress_plan(&target.items)
            || target.expected.is_some_and(|descriptor| {
                descriptor.source != target.source
                    || usize::from(descriptor.item_count) != target.items.len()
            })
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
        // 每个观察者守住自己的原五秒；胜者把同一截止和取消交给尚未poll的完整读取。
        let expires_at = match sample.now.checked_add(QUERY_LIMIT) {
            Some(deadline) => deadline,
            None => return Arc::new(Err(ConnectivityError::DeadlineExpired.into())),
        };
        let deadline = Arc::new(
            match ProgressExchangeDeadline::new(self.inner.clock.lease(sample.epoch), expires_at) {
                Ok(deadline) => deadline,
                Err(source) => return Arc::new(Err(source)),
            },
        );
        let round_cancel = CancellationToken::new();
        // 能力构造在状态锁外冻结会话；去重或过载时直接丢弃未启动的future，不提前执行IO。
        let round = self.inner.snapshot.clone().read_snapshot(
            peer,
            target.clone(),
            deadline.clone(),
            round_cancel.clone(),
        );
        let mut receive = match self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .query(peer, target, sample, intent, round_cancel, round)
        {
            Ok(receive) => receive,
            Err(reply) => return reply,
        };
        self.inner.wake.notify_one();
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
    /// 交接原负责人冻结的当前任务事实；None表示尚无中间值，重复交接不刷新原期限。
    /// 描述与实际权威逐字段一致后才可入队，不把旧快照归入重建后的producer。
    pub fn submit_activity(
        &self,
        peer: DeviceId,
        expected: ProgressTaskDescriptor,
        value: Option<ProgressActive>,
    ) -> Result<ProgressSubmission, ProgressReceiveError> {
        let source = expected.source;
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
        if task.descriptor != expected {
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
        if value.is_some_and(|value| !task.items.contains(&value.item)) {
            return Ok(ProgressSubmission::Refused(
                ProgressClaimRefusal::InvalidItem,
            ));
        }
        if value.map_or_else(
            || {
                task.items
                    .iter()
                    .all(|item| task.notification_closed.contains(item))
            },
            |value| task.notification_closed.contains(&value.item),
        ) {
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
        let submitted = match value {
            Some(value) => state.admit_live(peer, task.descriptor, value, sample),
            None => {
                let known = state
                    .live_tasks
                    .contains_key(&QueryKey::new(peer, task.descriptor));
                if !state.track_live(peer, task.descriptor, sample) {
                    ProgressSubmission::NotRetained
                } else if known {
                    ProgressSubmission::Unchanged
                } else {
                    ProgressSubmission::Retained
                }
            }
        };
        self.inner.wake.notify_one();
        Ok(submitted)
    }
}
