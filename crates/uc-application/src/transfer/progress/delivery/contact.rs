//! Contact与活动/终态共用实际交换；只有当前任务继续保持联系。
use super::super::clock::Sample;
use super::model::{Action, Job, QueryKey, Work};
use super::state::State;
use uc_core::file_transfer::{ExchangePriority, PROGRESS_CONTACT_GAP, TERMINAL_ROUND_LIMIT};

impl State {
    pub fn enqueue_contacts(&mut self, sample: Sample) {
        let mut peers = Vec::new();
        let mut candidates = Vec::new();
        for (key, task) in &self.live_tasks {
            if task.epoch == sample.epoch
                && !task.contact_queued
                && task.contact_due.is_some_and(|due| due <= sample.now)
                && !peers.contains(&key.peer)
            {
                peers.push(key.peer);
                candidates.push(*key);
            }
        }
        for key in candidates {
            if self
                .live_tasks
                .iter()
                .any(|(old, task)| old.peer == key.peer && task.contact_queued)
            {
                continue;
            }
            if self
                .queue
                .enqueue(key.peer, Work::Contact(key), ExchangePriority::Normal)
                .is_ok()
            {
                if let Some(task) = self.live_tasks.get_mut(&key) {
                    task.contact_queued = true;
                }
            }
        }
    }
    pub fn start_contact(&mut self, key: QueryKey, sample: Sample) -> Option<Job> {
        let task = self.live_tasks.get_mut(&key)?;
        task.contact_queued = false;
        if task.epoch != sample.epoch || task.contact_due.is_none_or(|due| due > sample.now) {
            return None;
        }
        let sequence = match task.contact_sequence.checked_add(1) {
            Some(sequence) => sequence,
            None => {
                task.contact_due = None;
                return None;
            }
        };
        let deadline = sample.now.checked_add(TERMINAL_ROUND_LIMIT)?;
        task.contact_sequence = sequence;
        task.contact_due = Some(sample.now.saturating_add(PROGRESS_CONTACT_GAP));
        let cancel = task.cancel.child_token();
        self.active.insert(
            key.peer,
            (Work::Contact(key), cancel.clone(), deadline, false),
        );
        Some(Job {
            work: Work::Contact(key),
            epoch: sample.epoch,
            deadline,
            cancel,
            action: Action::Contact {
                descriptor: task.descriptor,
                sequence,
            },
        })
    }
    pub fn finish_contact(
        &mut self,
        key: QueryKey,
        epoch: u64,
        permanent: bool,
        sample: Option<Sample>,
    ) {
        if self
            .active
            .get(&key.peer)
            .is_none_or(|(work, _, _, _)| *work != Work::Contact(key))
        {
            return;
        }
        self.active.remove(&key.peer);
        self.queue.complete(&key.peer);
        if let Some(task) = self.live_tasks.get_mut(&key) {
            if task.epoch != epoch {
                return;
            }
            if permanent {
                task.contact_due = None;
                task.cancel.cancel();
                return;
            }
            if task.contact_due.is_some() {
                if let Some(sample) = sample.filter(|sample| task.epoch == sample.epoch) {
                    task.contact_due = Some(sample.now.saturating_add(PROGRESS_CONTACT_GAP));
                }
            }
        }
    }
    pub fn remove_contact_task(&mut self, key: QueryKey) {
        self.queue.remove_queued(&key.peer, &Work::Contact(key));
        if let Some(task) = self.live_tasks.remove(&key) {
            task.cancel.cancel();
        }
    }
}
