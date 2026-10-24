use super::super::clock::Sample;
use super::super::{
    ElapsedClockError, ProgressReceiveError, ProgressSnapshotRead, ProgressSourceTask,
};
use super::model::{clock_failure, Action, Job, ProgressQueryResult, QueryKey, Work};
use super::state::State;
use crate::connectivity::ConnectivityError;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{ExchangePriority, ExchangeRefusal};
use uc_core::ids::DeviceId;

const MAX_QUERY_OBSERVERS: usize = 16;
pub(super) const QUERY_LIMIT: Duration = Duration::from_secs(5);
pub(super) struct QueryRecord {
    expected: ProgressSourceTask,
    epoch: u64,
    deadline: Duration,
    cancel: CancellationToken,
    reply: watch::Sender<Option<ProgressQueryResult>>,
    queued: bool,
    sealed: bool,
}
impl QueryRecord {
    fn complete(&mut self, result: ProgressQueryResult) {
        if !self.sealed {
            self.sealed = true;
            self.cancel.cancel();
            self.reply.send_replace(Some(result));
        }
    }
}
fn stopped() -> ProgressQueryResult {
    Arc::new(Err(ConnectivityError::Stopped.into()))
}
fn expired() -> ProgressQueryResult {
    Arc::new(Err(ConnectivityError::DeadlineExpired.into()))
}
impl State {
    pub fn query(
        &mut self,
        peer: DeviceId,
        expected: ProgressSourceTask,
        sample: Sample,
    ) -> Result<watch::Receiver<Option<ProgressQueryResult>>, ProgressQueryResult> {
        if self.closed {
            return Err(stopped());
        }
        let current = self.latest(sample);
        self.refresh(Some(current));
        if current.epoch != sample.epoch {
            return Err(clock_failure(None));
        }
        let deadline = sample
            .now
            .checked_add(QUERY_LIMIT)
            .filter(|deadline| *deadline > current.now)
            .ok_or_else(expired)?;
        let key = QueryKey::new(peer, expected.descriptor);
        if let Some(existing) = self.queries.get(&key) {
            if existing.expected.items != expected.items {
                return Err(Arc::new(Err(ProgressReceiveError::InvalidPlan)));
            }
            if let Some(reply) = existing.reply.borrow().clone() {
                return Err(reply);
            }
            if existing.reply.receiver_count() >= MAX_QUERY_OBSERVERS {
                return Err(Arc::new(Ok(ProgressSnapshotRead::Busy)));
            }
            return Ok(existing.reply.subscribe());
        }
        match self
            .queue
            .enqueue(peer, Work::Snapshot(key), ExchangePriority::Normal)
        {
            Ok(()) => {}
            Err(ExchangeRefusal::GlobalQueueFull(_) | ExchangeRefusal::PeerQueueFull(_)) => {
                return Err(Arc::new(Ok(ProgressSnapshotRead::Busy)))
            }
        }
        let (reply, receive) = watch::channel(None);
        self.queries.insert(
            key,
            QueryRecord {
                expected,
                epoch: sample.epoch,
                deadline,
                cancel: CancellationToken::new(),
                reply,
                queued: true,
                sealed: false,
            },
        );
        Ok(receive)
    }
    pub fn start_query(&mut self, key: QueryKey) -> Option<Job> {
        let record = self.queries.get_mut(&key)?;
        if record.sealed || record.reply.receiver_count() == 0 {
            return None;
        }
        record.queued = false;
        let cancel = record.cancel.clone();
        self.active.insert(
            key.peer,
            (Work::Snapshot(key), cancel.clone(), record.deadline, false),
        );
        Some(Job {
            work: Work::Snapshot(key),
            epoch: record.epoch,
            deadline: record.deadline,
            cancel,
            action: Action::Snapshot(record.expected.clone()),
        })
    }
    pub fn query_interested(&self, key: QueryKey) -> bool {
        self.queries
            .get(&key)
            .is_some_and(|record| !record.sealed && record.reply.receiver_count() > 0)
    }
    pub fn finish_query(
        &mut self,
        key: QueryKey,
        result: Result<ProgressSnapshotRead, ProgressReceiveError>,
    ) {
        if self
            .active
            .get(&key.peer)
            .is_none_or(|(work, _, _, _)| *work != Work::Snapshot(key))
        {
            return;
        }
        self.active.remove(&key.peer);
        self.queue.complete(&key.peer);
        if let Some(mut record) = self.queries.remove(&key) {
            record.complete(Arc::new(result));
        }
    }
    pub fn refresh_queries(&mut self, sample: Option<Sample>) {
        let retired: Vec<_> = self
            .queries
            .iter()
            .filter_map(|(key, record)| {
                if record.sealed {
                    None
                } else if sample.is_none_or(|sample| sample.epoch != record.epoch) {
                    Some((*key, clock_failure(None)))
                } else if sample.is_some_and(|sample| sample.now >= record.deadline) {
                    Some((*key, expired()))
                } else if record.reply.receiver_count() == 0 {
                    Some((*key, stopped()))
                } else {
                    None
                }
            })
            .collect();
        for (key, reply) in retired {
            self.seal_query(key, reply);
        }
    }
    pub fn retire_queries(&mut self, epoch: Option<u64>, source: Option<Arc<ElapsedClockError>>) {
        let retired: Vec<_> = self
            .queries
            .iter()
            .filter(|(_, record)| epoch.is_none_or(|epoch| record.epoch < epoch))
            .map(|(key, _)| *key)
            .collect();
        let reply = clock_failure(source);
        for key in retired {
            self.seal_query(key, reply.clone());
        }
    }
    pub fn close_queries(&mut self) {
        let keys: Vec<_> = self.queries.keys().copied().collect();
        let reply = stopped();
        for key in keys {
            self.seal_query(key, reply.clone());
        }
    }
    fn seal_query(&mut self, key: QueryKey, reply: ProgressQueryResult) {
        let Some(record) = self.queries.get_mut(&key) else {
            return;
        };
        record.complete(reply);
        if record.queued {
            self.queue.remove_queued(&key.peer, &Work::Snapshot(key));
            self.queries.remove(&key);
        }
        // 活动记录留到实际finish：重复等待者只能观察同一封口结果，不能重建预算。
    }
}
