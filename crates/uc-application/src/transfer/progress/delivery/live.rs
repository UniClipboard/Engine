//! 最新中间值独立有界保留；任务节流和已见序号不随缓存淘汰重置。
use super::super::{ProgressClaimRefusal, ProgressSubmission, ProgressTaskDescriptor};
use super::clock::Sample;
use super::model::{Action, Job, Key, QueryKey, Work};
use super::state::State;
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ExchangePriority, ProgressActive, ProgressItem, ProgressItemEffect, ProgressItemInput,
    ProgressItemProjection, ProgressTerminal, TERMINAL_ROUND_LIMIT,
};
use uc_core::ids::DeviceId;

const LIVE_GLOBAL: usize = 256;
const LIVE_PEER: usize = 64;
const LIVE_GAP: Duration = Duration::from_millis(200);
pub(super) struct LiveTask {
    pub descriptor: ProgressTaskDescriptor,
    epoch: u64,
    next: Duration,
    seen: HashMap<ProgressItem, ProgressItemProjection>,
}
pub(super) struct LiveEntry {
    descriptor: ProgressTaskDescriptor,
    value: ProgressActive,
    epoch: u64,
    dirty: bool,
    queued: bool,
    cancel: CancellationToken,
}
impl State {
    pub fn live_prune_candidates(
        &self,
        peer: DeviceId,
        descriptor: ProgressTaskDescriptor,
    ) -> Vec<(QueryKey, ProgressTaskDescriptor)> {
        let key = QueryKey::new(peer, descriptor);
        if self.live_tasks.contains_key(&key)
            || (self.live_tasks.len() < LIVE_GLOBAL
                && self
                    .live_tasks
                    .keys()
                    .filter(|key| key.peer == peer)
                    .count()
                    < LIVE_PEER)
        {
            return Vec::new();
        }
        self.live_tasks
            .iter()
            .map(|(key, task)| (*key, task.descriptor))
            .collect()
    }
    pub fn prune_live_tasks(&mut self, obsolete: &[QueryKey]) {
        for key in obsolete {
            let removed: Vec<_> = self
                .live
                .iter()
                .filter(|(item, entry)| QueryKey::new(item.peer, entry.descriptor) == *key)
                .map(|(item, _)| *item)
                .collect();
            for item in removed {
                self.remove_live(item);
            }
            self.live_tasks.remove(key);
        }
    }
    pub fn admit_live(
        &mut self,
        peer: DeviceId,
        descriptor: ProgressTaskDescriptor,
        value: ProgressActive,
        sample: Sample,
    ) -> ProgressSubmission {
        if self.closed {
            return ProgressSubmission::Closed;
        }
        let current = self.latest(sample);
        if current.epoch != sample.epoch {
            return ProgressSubmission::NotRetained;
        }
        let scope = QueryKey::new(peer, descriptor);
        if !self.live_tasks.contains_key(&scope) {
            if self.live_tasks.len() >= LIVE_GLOBAL
                || self
                    .live_tasks
                    .keys()
                    .filter(|key| key.peer == peer)
                    .count()
                    >= LIVE_PEER
            {
                return ProgressSubmission::NotRetained;
            }
            self.live_tasks.insert(
                scope,
                LiveTask {
                    descriptor,
                    epoch: sample.epoch,
                    next: current.now,
                    seen: HashMap::new(),
                },
            );
        }
        let Some(task) = self.live_tasks.get_mut(&scope) else {
            return ProgressSubmission::NotRetained;
        };
        if task.epoch != sample.epoch {
            task.epoch = sample.epoch;
            task.next = current.now.saturating_add(LIVE_GAP);
        }
        match task
            .seen
            .entry(value.item)
            .or_default()
            .apply(ProgressItemInput::Update(value))
        {
            ProgressItemEffect::Duplicate => return ProgressSubmission::Unchanged,
            ProgressItemEffect::Stale | ProgressItemEffect::Conflict => {
                return ProgressSubmission::Refused(ProgressClaimRefusal::StaleUpdate)
            }
            ProgressItemEffect::Invalid => {
                return ProgressSubmission::Refused(ProgressClaimRefusal::InvalidUpdate)
            }
            ProgressItemEffect::Adopted => {}
        }
        let key = Key::live(peer, descriptor, value.item);
        self.live_order.retain(|old| *old != key);
        self.live_order.push_back(key);
        if let Some(entry) = self.live.get_mut(&key) {
            entry.value = value;
            entry.dirty = true;
        } else {
            self.live.insert(
                key,
                LiveEntry {
                    descriptor,
                    value,
                    epoch: sample.epoch,
                    dirty: true,
                    queued: false,
                    cancel: CancellationToken::new(),
                },
            );
        }
        while self.live.keys().filter(|key| key.peer == peer).count() > LIVE_PEER {
            let oldest = self.live_order.iter().find(|key| key.peer == peer).copied();
            if let Some(oldest) = oldest {
                self.remove_live(oldest);
            } else {
                break;
            }
        }
        while self.live.len() > LIVE_GLOBAL {
            if let Some(oldest) = self.live_order.front().copied() {
                self.remove_live(oldest);
            } else {
                break;
            }
        }
        if self.live.contains_key(&key) {
            ProgressSubmission::Retained
        } else {
            ProgressSubmission::NotRetained
        }
    }
    pub fn refresh_live(&mut self, sample: Option<Sample>) {
        let keys: Vec<_> = self
            .live
            .iter()
            .filter(|(_, entry)| sample.is_none_or(|sample| sample.epoch != entry.epoch))
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            self.remove_live(key);
        }
    }
    pub fn retire_live(&mut self, epoch: Option<u64>) {
        let keys: Vec<_> = self
            .live
            .iter()
            .filter(|(_, entry)| epoch.is_none_or(|epoch| entry.epoch < epoch))
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            self.remove_live(key);
        }
    }
    pub fn enqueue_live(&mut self, sample: Sample) {
        // 保留缓存不是等待队列；每端最多放一个中间候选，终态已先进入同一队列。
        let mut peers = Vec::new();
        for key in &self.live_order {
            if !peers.contains(&key.peer) {
                peers.push(key.peer);
            }
        }
        for peer in peers {
            if self
                .live
                .iter()
                .any(|(key, entry)| key.peer == peer && entry.queued)
            {
                continue;
            }
            let candidate = self
                .live_order
                .iter()
                .find(|key| {
                    key.peer == peer
                        && self.live.get(key).is_some_and(|entry| {
                            entry.dirty
                                && !entry.queued
                                && self
                                    .live_tasks
                                    .get(&QueryKey::new(peer, entry.descriptor))
                                    .is_some_and(|task| {
                                        task.epoch == sample.epoch && task.next <= sample.now
                                    })
                        })
                })
                .copied();
            if let Some(key) = candidate {
                if self
                    .queue
                    .enqueue(peer, Work::Live(key), ExchangePriority::Normal)
                    .is_ok()
                {
                    if let Some(entry) = self.live.get_mut(&key) {
                        entry.queued = true;
                    }
                }
            }
        }
    }
    pub fn start_live(&mut self, key: Key, sample: Sample) -> Option<Job> {
        let entry = self.live.get_mut(&key)?;
        entry.queued = false;
        let task = self
            .live_tasks
            .get_mut(&QueryKey::new(key.peer, entry.descriptor))?;
        if !entry.dirty || task.epoch != sample.epoch || task.next > sample.now {
            return None;
        }
        let deadline = sample.now.checked_add(TERMINAL_ROUND_LIMIT)?;
        task.next = sample.now.saturating_add(LIVE_GAP);
        entry.dirty = false;
        let cancel = entry.cancel.child_token();
        self.active
            .insert(key.peer, (Work::Live(key), cancel.clone(), deadline, false));
        Some(Job {
            work: Work::Live(key),
            epoch: sample.epoch,
            deadline,
            cancel,
            action: Action::Live {
                descriptor: entry.descriptor,
                value: entry.value,
            },
        })
    }
    pub fn finish_live(
        &mut self,
        key: Key,
        sent: ProgressActive,
        permanent: bool,
        sample: Option<Sample>,
    ) {
        if self
            .active
            .get(&key.peer)
            .is_none_or(|(work, _, _, _)| *work != Work::Live(key))
        {
            return;
        }
        self.active.remove(&key.peer);
        self.queue.complete(&key.peer);
        // 从实际结束计冷却，避免登记或流写入延迟后连续发送两份中间值。
        if let Some(sample) = sample {
            if let Some(task) = self.live_tasks.iter_mut().find_map(|(scope, task)| {
                (Key::live(scope.peer, task.descriptor, sent.item) == key).then_some(task)
            }) {
                if task.epoch == sample.epoch {
                    task.next = task.next.max(sample.now.saturating_add(LIVE_GAP));
                }
            }
        }
        if permanent {
            self.remove_live(key);
        } else if let Some(entry) = self.live.get_mut(&key) {
            // 新值已经在提交时置dirty；旧轮结束不能抹掉期间到达的新值。
            if entry.value == sent {
                entry.dirty = false;
            }
        }
    }
    pub fn settled_live(
        &mut self,
        peer: DeviceId,
        descriptor: ProgressTaskDescriptor,
        result: ProgressTerminal,
    ) {
        if let Some(task) = self.live_tasks.get_mut(&QueryKey::new(peer, descriptor)) {
            task.seen
                .entry(result.item)
                .or_default()
                .apply(ProgressItemInput::Terminal(result));
        }
        self.remove_live(Key::live(peer, descriptor, result.item));
    }
    pub fn close_live(&mut self) {
        let keys: Vec<_> = self.live.keys().copied().collect();
        for key in keys {
            self.remove_live(key);
        }
        self.live_tasks.clear();
    }
    pub fn recover_live(&mut self, peer: Option<DeviceId>) {
        for (key, entry) in &mut self.live {
            if peer.is_none_or(|peer| peer == key.peer) {
                entry.dirty = true;
            }
        }
    }
    pub fn pending_live(&self) -> bool {
        self.live.values().any(|entry| entry.dirty)
    }
    fn remove_live(&mut self, key: Key) {
        if let Some(entry) = self.live.remove(&key) {
            entry.cancel.cancel();
            if entry.queued {
                self.queue.remove_queued(&key.peer, &Work::Live(key));
            }
        }
        self.live_order.retain(|old| *old != key);
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
