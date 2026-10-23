use crate::transfer::policy::{
    ACTIVE_DOWNLOADS, ACTIVE_DOWNLOADS_PER_PEER, QUEUED_DOWNLOADS, QUEUED_DOWNLOADS_PER_PEER,
    QUEUE_TIMEOUT,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
// 排队前只需要票据中的认证公钥；业务 DeviceId 仍由当前权威目录解析。
type DownloadPeer = [u8; 32];
use uc_observability_contract::wire_enum;
#[cfg(feature = "test-support")]
mod test_support;
#[cfg(feature = "test-support")]
use test_support::{record_snapshot, ResourceEvidenceId, SnapshotEvent};

wire_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    pub(in super::super::super) enum ScheduleRefusal: &'static str {
        #[error("blob download capacity exhausted")]
        Busy => "blob_download_busy",
        #[error("blob download queue deadline expired")]
        QueueExpired => "blob_download_queue_expired",
        #[error("blob download cancelled")]
        Cancelled => "blob_download_cancelled",
        #[error("blob download owner closed")]
        Closed => "blob_download_closed",
        #[error("blob download identifiers exhausted")]
        IdentifiersExhausted => "blob_download_identifiers_exhausted",
    }
}

#[derive(Debug, thiserror::Error)]
#[error("blob download admission refused")]
pub(in super::super::super) struct ScheduleError(#[source] pub ScheduleRefusal);

impl ErrorClass for ScheduleError {
    fn class(&self) -> &'static str {
        self.0.wire()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Queued,
    Running(Instant),
    Failed(ScheduleRefusal),
}
struct Slot {
    id: u64,
    peer: DownloadPeer,
    accepted_at: Instant,
    phase: Mutex<Phase>,
    changed: Notify,
}
impl Slot {
    fn phase(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase;
        self.changed.notify_one();
    }
}

#[derive(Default)]
struct State {
    #[cfg(feature = "test-support")]
    resource_evidence: ResourceEvidenceId,
    changed: Arc<Notify>,
    slots: HashMap<u64, Arc<Slot>>,
    pending: HashMap<DownloadPeer, VecDeque<u64>>,
    rotation: VecDeque<DownloadPeer>,
    running: HashMap<DownloadPeer, usize>,
    next_id: u64,
    closed: bool,
}
impl State {
    fn can_start(&self, peer: &DownloadPeer) -> bool {
        self.running.values().sum::<usize>() < ACTIVE_DOWNLOADS
            && self.running.get(peer).copied().unwrap_or(0) < ACTIVE_DOWNLOADS_PER_PEER
    }

    fn remove(&mut self, id: u64, failure: Option<ScheduleRefusal>) {
        let Some(slot) = self.slots.remove(&id) else {
            return;
        };
        match slot.phase() {
            Phase::Running(_) => {
                if let Some(count) = self.running.get_mut(&slot.peer) {
                    *count -= 1;
                    if *count == 0 {
                        self.running.remove(&slot.peer);
                    }
                }
            }
            Phase::Queued => {
                if let Some(queue) = self.pending.get_mut(&slot.peer) {
                    queue.retain(|queued| *queued != id);
                    if queue.is_empty() {
                        self.pending.remove(&slot.peer);
                        self.rotation.retain(|peer| peer != &slot.peer);
                    }
                }
            }
            Phase::Failed(_) => {}
        }
        if let Some(reason) = failure {
            slot.set_phase(Phase::Failed(reason));
        }
    }

    fn dispatch(&mut self, now: Instant) {
        // 已到期项不能因为自身 future 尚未被 poll 而占住或刷新等待名额。
        let expired: Vec<_> = self
            .slots
            .values()
            .filter(|slot| slot.phase() == Phase::Queued && now >= slot.accepted_at + QUEUE_TIMEOUT)
            .map(|slot| slot.id)
            .collect();
        for id in expired {
            self.remove(id, Some(ScheduleRefusal::QueueExpired));
        }
        if self.closed {
            return;
        }
        while self.running.values().sum::<usize>() < ACTIVE_DOWNLOADS {
            let mut started = false;
            for _ in 0..self.rotation.len() {
                let Some(peer) = self.rotation.pop_front() else {
                    break;
                };
                if !self.can_start(&peer) {
                    self.rotation.push_back(peer);
                    continue;
                }
                let id = self.pending.get_mut(&peer).and_then(VecDeque::pop_front);
                if self.pending.get(&peer).is_some_and(VecDeque::is_empty) {
                    self.pending.remove(&peer);
                } else {
                    self.rotation.push_back(peer.clone());
                }
                if let Some(slot) = id.and_then(|id| self.slots.get(&id)) {
                    *self.running.entry(peer).or_default() += 1;
                    slot.set_phase(Phase::Running(now));
                    started = true;
                    break;
                }
            }
            if !started {
                break;
            }
        }
    }
}

#[derive(Clone)]
pub(in super::super::super) struct DownloadScheduler {
    state: Arc<Mutex<State>>,
}

impl Default for DownloadScheduler {
    fn default() -> Self {
        let scheduler = Self {
            state: Arc::default(),
        };
        #[cfg(feature = "test-support")]
        record_snapshot(
            &scheduler
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            SnapshotEvent::Created,
        );
        scheduler
    }
}

/// 唯一逻辑项的额度；不可克隆，实际任务及本地结算完成后才允许丢弃。
pub(in super::super::super) struct DownloadPermit {
    pub started_at: Instant,
    _reservation: DownloadReservation,
}
pub(in super::super::super) struct DownloadReservation {
    state: Arc<Mutex<State>>,
    slot: Arc<Slot>,
}
impl Drop for DownloadReservation {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.remove(self.slot.id, None);
        state.dispatch(Instant::now());
        #[cfg(feature = "test-support")]
        record_snapshot(&state, SnapshotEvent::Returned);
        let changed = state.changed.clone();
        drop(state);
        changed.notify_waiters();
    }
}

impl DownloadScheduler {
    /// 关闭后仍等待原许可的实际持有者，不因网络任务先退出而丢弃本地结算。
    pub async fn wait_settled(&self) {
        let changed = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .changed
            .clone();
        loop {
            let notified = changed.notified();
            let settled = {
                let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if state.slots.is_empty() {
                    #[cfg(feature = "test-support")]
                    record_snapshot(&state, SnapshotEvent::Settled);
                    true
                } else {
                    false
                }
            };
            if settled {
                return;
            }
            notified.await;
        }
    }

    pub fn reserve(
        &self,
        peer: DownloadPeer,
        cancel: &CancellationToken,
    ) -> Result<DownloadReservation, ScheduleError> {
        if cancel.is_cancelled() {
            return Err(ScheduleError(ScheduleRefusal::Cancelled));
        }
        let reservation = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            state.dispatch(now);
            if state.closed {
                return Err(ScheduleError(ScheduleRefusal::Closed));
            }
            let queued = state.pending.values().map(VecDeque::len).sum::<usize>();
            let peer_queued = state.pending.get(&peer).map_or(0, VecDeque::len);
            if !state.can_start(&peer)
                && (queued >= QUEUED_DOWNLOADS || peer_queued >= QUEUED_DOWNLOADS_PER_PEER)
            {
                return Err(ScheduleError(ScheduleRefusal::Busy));
            }
            let id = state.next_id;
            state.next_id = id
                .checked_add(1)
                .ok_or(ScheduleError(ScheduleRefusal::IdentifiersExhausted))?;
            let slot = Arc::new(Slot {
                id,
                peer: peer.clone(),
                accepted_at: now,
                phase: Mutex::new(Phase::Queued),
                changed: Notify::new(),
            });
            state.slots.insert(id, slot.clone());
            if !state.pending.contains_key(&peer) {
                state.rotation.push_back(peer.clone());
            }
            state.pending.entry(peer).or_default().push_back(id);
            state.dispatch(now);
            #[cfg(feature = "test-support")]
            record_snapshot(&state, SnapshotEvent::Reserved);
            DownloadReservation {
                state: self.state.clone(),
                slot,
            }
        };
        Ok(reservation)
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        let pending: Vec<_> = state
            .slots
            .values()
            .filter(|slot| slot.phase() == Phase::Queued)
            .map(|slot| slot.id)
            .collect();
        for id in pending {
            state.remove(id, Some(ScheduleRefusal::Closed));
        }
        #[cfg(feature = "test-support")]
        record_snapshot(&state, SnapshotEvent::Closed);
        // 运行中许可仍归实际任务，关闭不虚放额度；完整负责人另行发出停止意图。
    }
}

impl DownloadReservation {
    pub async fn wait(self, cancel: &CancellationToken) -> Result<DownloadPermit, ScheduleError> {
        let reservation = self;
        loop {
            if cancel.is_cancelled() {
                return Err(ScheduleError(ScheduleRefusal::Cancelled));
            }
            match reservation.slot.phase() {
                Phase::Running(started_at) => {
                    return Ok(DownloadPermit {
                        started_at,
                        _reservation: reservation,
                    });
                }
                Phase::Failed(reason) => return Err(ScheduleError(reason)),
                Phase::Queued => {}
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ScheduleError(ScheduleRefusal::Cancelled)),
                _ = sleep_until(reservation.slot.accepted_at + QUEUE_TIMEOUT) => {
                    let mut state = reservation.state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.dispatch(Instant::now());
                }
                _ = reservation.slot.changed.notified() => {}
            }
        }
    }
}
