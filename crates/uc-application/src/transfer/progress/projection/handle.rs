use super::super::{
    ElapsedClockPort, ProgressInboundWirePort, ProgressProjectionSnapshot, ProgressReceiveError,
    TransferProgressReceiver, TransferProgressTransport,
};
use super::model::{ProgressProjectionAdmission, ProgressProjectionEvent, ProgressProjectionKey};
use super::worker::ProgressProjectionWorker;
use crate::connectivity::{ConnectivityError, ConnectivityManager};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{broadcast, Notify};
use uc_core::file_transfer::{
    valid_progress_plan, ExchangeQueue, ProgressIdentity, ProgressItem, ProgressProjectionCapacity,
    ProjectionCapacityInput, ProjectionRetention,
};
use uc_core::ids::DeviceId;

pub(super) struct Slot {
    pub receiver: Arc<TransferProgressReceiver>,
    pub published: u64,
}
#[derive(Default)]
pub(super) struct State {
    connection_generation: u64,
    pub slots: HashMap<ProgressProjectionKey, Slot>,
    pub capacity: ProgressProjectionCapacity<DeviceId, ProgressProjectionKey>,
    pub registrations: ExchangeQueue<DeviceId, ()>,
}
pub(super) struct Inner {
    pub state: Mutex<State>,
    pub clock: Arc<dyn ElapsedClockPort>,
    pub events: broadcast::Sender<ProgressProjectionEvent>,
    pub wake: Notify,
}
#[derive(Clone)]
pub struct ProgressProjectionRegistry {
    pub(super) inner: Arc<Inner>,
}
impl ProgressProjectionRegistry {
    /// 调用方持有并等待worker；封口后仍等已开始的登记能力实际返回。
    pub fn new(clock: Arc<dyn ElapsedClockPort>) -> (Self, ProgressProjectionWorker) {
        let (events, _) = broadcast::channel(128);
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            clock,
            events,
            wake: Notify::new(),
        });
        (
            Self {
                inner: inner.clone(),
            },
            ProgressProjectionWorker { inner },
        )
    }
    /// 入站连接与同一投影共用世代命名空间；重建transport不能令旧关联重新获得资格。
    pub fn inbound_transport(
        &self,
        manager: ConnectivityManager,
        wire: Arc<dyn ProgressInboundWirePort>,
        clock: Arc<dyn ElapsedClockPort>,
    ) -> Arc<TransferProgressTransport> {
        TransferProgressTransport::new(manager, wire, clock, self.clone())
    }
    pub(crate) fn connection_generation(&self) -> Result<u64, ProgressReceiveError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.capacity.is_closed() {
            return Err(ConnectivityError::Stopped.into());
        }
        let generation = state
            .connection_generation
            .checked_add(1)
            .ok_or(ConnectivityError::Stopped)?;
        state.connection_generation = generation;
        Ok(generation)
    }
    /// 仅由原完整发送负责人接受意图时调用；网络帧不得以此创建任务。
    pub fn accept_intent(
        &self,
        key: ProgressProjectionKey,
        plan: Vec<ProgressItem>,
    ) -> Result<ProgressProjectionAdmission, ProgressReceiveError> {
        if !valid_progress_plan(&plan) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.capacity.is_closed() {
            return Ok(ProgressProjectionAdmission::Closed);
        }
        if let Some(slot) = state.slots.get(&key) {
            return if slot.receiver.matches_plan(&plan) {
                Ok(ProgressProjectionAdmission::Unchanged)
            } else {
                Err(ProgressReceiveError::InvalidPlan)
            };
        }
        let receiver = Arc::new(TransferProgressReceiver::new(
            key.peer,
            key.source,
            plan,
            self.inner.clock.clone(),
        )?);
        match state.capacity.apply(ProjectionCapacityInput::Retain {
            peer: key.peer,
            key,
        }) {
            ProjectionRetention::Inserted { evicted } => {
                if let Some(old) = evicted {
                    retire(&self.inner, &mut state, old);
                }
                state.slots.insert(
                    key,
                    Slot {
                        published: receiver.revision(),
                        receiver,
                    },
                );
                let _ = self
                    .inner
                    .events
                    .send(ProgressProjectionEvent::Changed(key));
                self.inner.wake.notify_one();
                Ok(ProgressProjectionAdmission::Retained)
            }
            ProjectionRetention::Closed => Ok(ProgressProjectionAdmission::Closed),
            ProjectionRetention::Unchanged | ProjectionRetention::Retired(_) => {
                Err(ProgressReceiveError::Stale)
            }
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<ProgressProjectionEvent> {
        self.inner.events.subscribe()
    }
    pub fn snapshot(&self, key: ProgressProjectionKey) -> Option<ProgressProjectionSnapshot> {
        let receiver = self.receiver(key)?;
        let view = receiver.snapshot();
        self.current(key, &receiver).then_some(view)
    }
    /// 只返回原发送意图已经授权且仍保留的任务，不枚举任意本地条目。
    pub fn snapshots(&self) -> Vec<(ProgressProjectionKey, ProgressProjectionSnapshot)> {
        let keys: Vec<_> = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slots
            .keys()
            .copied()
            .collect();
        keys.into_iter()
            .filter_map(|key| self.snapshot(key).map(|snapshot| (key, snapshot)))
            .collect()
    }
    pub fn remove(&self, key: ProgressProjectionKey) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.capacity.apply(ProjectionCapacityInput::Remove(key));
        retire(&self.inner, &mut state, key);
    }
    /// 删除源条目时一次退休全部接收设备的关联，调用方不枚举内部投影。
    pub fn remove_source(&self, source: ProgressIdentity) {
        self.remove_matching(|key| key.source == source);
    }
    /// 成员退出时一次退休该端全部关联，不推断文件实际取消结果。
    pub fn remove_peer(&self, peer: DeviceId) {
        self.remove_matching(|key| key.peer == peer);
    }
    fn remove_matching(&self, select: impl Fn(&ProgressProjectionKey) -> bool) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let keys: Vec<_> = state
            .slots
            .keys()
            .filter(|key| select(key))
            .copied()
            .collect();
        for key in keys {
            state.capacity.apply(ProjectionCapacityInput::Remove(key));
            retire(&self.inner, &mut state, key);
        }
        self.inner.wake.notify_one();
    }
    pub fn clear(&self) {
        self.retire_all(false);
    }
    /// 同步封口，拒绝后续采用；等待worker返回后才表示已开始的登记全部结束。
    pub fn close(&self) {
        self.retire_all(true);
    }
    fn retire_all(&self, close: bool) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let input = if close {
            ProjectionCapacityInput::Close
        } else {
            ProjectionCapacityInput::Clear
        };
        if let ProjectionRetention::Retired(keys) = state.capacity.apply(input) {
            for key in keys {
                retire(&self.inner, &mut state, key);
            }
            // 首次封口才唤醒；重复close不能让等待实际登记结束的worker空转。
            self.inner.wake.notify_one();
        }
    }
    pub(super) fn receiver(
        &self,
        key: ProgressProjectionKey,
    ) -> Option<Arc<TransferProgressReceiver>> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slots
            .get(&key)
            .map(|slot| slot.receiver.clone())
    }
    pub(super) fn current(
        &self,
        key: ProgressProjectionKey,
        receiver: &Arc<TransferProgressReceiver>,
    ) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slots
            .get(&key)
            .is_some_and(|slot| Arc::ptr_eq(&slot.receiver, receiver))
    }
    pub(super) fn publish(
        &self,
        key: ProgressProjectionKey,
        receiver: &Arc<TransferProgressReceiver>,
    ) -> bool {
        let version = receiver.revision();
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(slot) = state
            .slots
            .get_mut(&key)
            .filter(|slot| Arc::ptr_eq(&slot.receiver, receiver))
        else {
            return false;
        };
        if slot.published < version {
            slot.published = version;
            self.inner.wake.notify_one();
            let _ = self
                .inner
                .events
                .send(ProgressProjectionEvent::Changed(key));
        }
        true
    }
}
fn retire(inner: &Inner, state: &mut State, key: ProgressProjectionKey) {
    if let Some(slot) = state.slots.remove(&key) {
        slot.receiver.retire();
        let _ = inner.events.send(ProgressProjectionEvent::Removed(key));
    }
}
