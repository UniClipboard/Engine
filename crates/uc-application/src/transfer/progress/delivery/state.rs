use super::super::{ElapsedClockError, ProgressTaskDescriptor};
use super::clock::Sample;
use super::model::{Action, Entry, Job, Key, QueryKey, Work};
use super::query::QueryRecord;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ExchangePriority, ExchangeQueue, ProgressTerminal, RetransmitEffect, RetransmitInput,
    TerminalLedger, TerminalRetransmit, EXCHANGE_GLOBAL_QUEUED,
};
use uc_core::ids::DeviceId;

#[derive(Default)]
pub(super) struct State {
    pub closed: bool,
    pub(super) observed: Option<Sample>,
    pub(super) entries: HashMap<Key, Entry>,
    pub(super) ledger: TerminalLedger<DeviceId, Key>,
    pub(super) queue: ExchangeQueue<DeviceId, Work>,
    pub(super) peers: VecDeque<DeviceId>,
    pub(super) active: HashMap<DeviceId, (Work, CancellationToken, Duration, bool)>,
    pub(super) queries: HashMap<QueryKey, QueryRecord>,
}
impl State {
    pub fn admit(
        &mut self,
        key: Key,
        descriptor: ProgressTaskDescriptor,
        result: ProgressTerminal,
        sample: Sample,
    ) -> bool {
        if self
            .observed
            .is_some_and(|current| current.epoch > sample.epoch)
        {
            return false;
        }
        let current = self.latest(sample);
        let mut retry = TerminalRetransmit::new(Some(sample.now));
        if matches!(
            retry.apply(RetransmitInput::Check(Some(current.now))),
            RetransmitEffect::Stopped(_)
        ) {
            return false;
        }
        if !self.peers.contains(&key.peer) {
            self.peers.push_back(key.peer);
        }
        self.entries.insert(
            key,
            Entry {
                descriptor,
                result,
                generated: sample.now,
                epoch: sample.epoch,
                cancel: CancellationToken::new(),
                retry,
                queued: false,
            },
        );
        for (_, evicted) in self.ledger.admit(key.peer, key, sample.now) {
            self.remove(evicted);
        }
        self.entries.contains_key(&key)
    }
    pub(super) fn latest(&mut self, sample: Sample) -> Sample {
        let latest = self
            .observed
            .filter(|current| {
                current.epoch > sample.epoch
                    || (current.epoch == sample.epoch && current.now > sample.now)
            })
            .unwrap_or(sample);
        self.observed = Some(latest);
        latest
    }
    pub fn retire_before(&mut self, epoch: Option<u64>, source: Option<Arc<ElapsedClockError>>) {
        self.retire_queries(epoch, source);
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| epoch.is_none_or(|epoch| entry.epoch < epoch))
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            self.remove(key);
        }
    }
    pub fn refresh(&mut self, sample: Option<Sample>) {
        let sample = sample.map(|sample| self.latest(sample));
        if let Some(sample) = sample {
            for (_, cancel, deadline, timed_out) in self.active.values_mut() {
                if sample.now >= *deadline {
                    *timed_out = true;
                    cancel.cancel();
                }
            }
        }
        self.refresh_queries(sample);
        let retired: Vec<_> = self
            .entries
            .iter_mut()
            .filter_map(|(key, entry)| {
                let now = sample
                    .filter(|sample| sample.epoch == entry.epoch)
                    .map(|sample| sample.now);
                matches!(
                    entry.retry.apply(RetransmitInput::Check(now)),
                    RetransmitEffect::Stopped(_)
                )
                .then_some(*key)
            })
            .collect();
        for key in retired {
            self.remove(key);
        }
        self.peers
            .retain(|peer| self.entries.keys().any(|key| key.peer == *peer));
    }
    pub fn dispatch(&mut self, sample: Sample) -> Vec<Job> {
        let sample = self.latest(sample);
        self.refresh(Some(sample));
        // 每端只放一个交换候选；未获交换额度的结果仍只占独立的有界保留缓存。
        for _ in 0..self.peers.len() {
            let Some(peer) = self.peers.pop_front() else {
                break;
            };
            self.peers.push_back(peer);
            if self
                .entries
                .iter()
                .any(|(key, entry)| key.peer == peer && entry.queued)
            {
                continue;
            }
            let candidate = self
                .entries
                .iter_mut()
                .filter(|(key, entry)| key.peer == peer && !entry.queued)
                .filter_map(|(key, entry)| {
                    (entry.retry.apply(RetransmitInput::Check(Some(sample.now)))
                        == RetransmitEffect::Ready)
                        .then_some((*key, entry.generated))
                })
                .min_by_key(|(_, generated)| *generated)
                .map(|(key, _)| key);
            if let Some(key) = candidate {
                if self
                    .queue
                    .enqueue(peer, Work::Terminal(key), ExchangePriority::Terminal)
                    .is_ok()
                {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.queued = true;
                    }
                } else if self.queue.queued() >= EXCHANGE_GLOBAL_QUEUED {
                    break;
                }
            }
        }
        let mut jobs = Vec::new();
        while let Some((peer, work)) = self.queue.next_ready() {
            let key = match work {
                Work::Terminal(key) => key,
                Work::Snapshot(key) => {
                    if let Some(job) = self.start_query(key) {
                        jobs.push(job);
                    } else {
                        self.queue.complete(&peer);
                    }
                    continue;
                }
            };
            let Some(entry) = self.entries.get_mut(&key) else {
                self.queue.complete(&peer);
                continue;
            };
            entry.queued = false;
            match entry.retry.apply(RetransmitInput::Start(Some(sample.now))) {
                RetransmitEffect::Start { deadline } => {
                    let cancel = entry.cancel.child_token();
                    self.active
                        .insert(peer, (work, cancel.clone(), deadline, false));
                    jobs.push(Job {
                        work,
                        action: Action::Terminal {
                            descriptor: entry.descriptor,
                            result: entry.result,
                        },
                        epoch: entry.epoch,
                        deadline,
                        cancel,
                    });
                }
                _ => self.queue.complete(&peer),
            }
        }
        jobs
    }
    pub fn finish(&mut self, key: Key, acknowledged: bool, retryable: bool, cancelled: bool) {
        if self
            .active
            .get(&key.peer)
            .is_none_or(|(active, _, _, _)| *active != Work::Terminal(key))
        {
            return;
        }
        let timed_out = self
            .active
            .get(&key.peer)
            .is_some_and(|(_, _, _, timed_out)| *timed_out);
        let retryable = retryable || (cancelled && timed_out);
        self.active.remove(&key.peer);
        self.queue.complete(&key.peer);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.retry.apply(RetransmitInput::RoundFinished);
            if acknowledged {
                entry.retry.apply(RetransmitInput::Acknowledged);
            }
        }
        if acknowledged || !retryable {
            self.remove(key);
        }
    }
    pub fn recovered(&mut self, peer: Option<DeviceId>) {
        for (key, entry) in &mut self.entries {
            if peer.is_none_or(|peer| key.peer == peer) {
                entry.retry.apply(RetransmitInput::NetworkRecovered);
            }
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.close_queries();
        let keys: Vec<_> = self.entries.keys().copied().collect();
        for key in keys {
            self.remove(key);
        }
        for (_, cancel, _, _) in self.active.values() {
            cancel.cancel();
        }
    }
    pub fn has_work(&self) -> bool {
        !self.entries.is_empty() || !self.active.is_empty() || !self.queries.is_empty()
    }
    fn remove(&mut self, key: Key) {
        if let Some(entry) = self.entries.remove(&key) {
            entry.cancel.cancel();
            if entry.queued {
                self.queue.remove_queued(&key.peer, &Work::Terminal(key));
            }
            self.ledger.release(&key.peer, &key);
            if !self
                .entries
                .keys()
                .any(|candidate| candidate.peer == key.peer)
            {
                self.peers.retain(|peer| *peer != key.peer);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
