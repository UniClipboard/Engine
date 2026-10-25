//! 同一接收权威下的运行期完整快照；内容修订独立于登记修订，随原目录有界回收。
use super::ProgressSourceCatalog;
use crate::transfer::progress::{
    ProgressReceiveError, ProgressSnapshot, ProgressSourceTask, ProgressTaskDescriptor,
};
use std::sync::Arc;
use uc_core::file_transfer::{ProgressIdentity, ProgressItemState};
use uc_core::ids::{DeviceId, EntryId};

impl ProgressSourceCatalog {
    pub(crate) fn query_scope(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<(EntryId, ProgressSourceTask)> {
        self.tasks
            .get(&(*peer, source))
            .map(|entry| (entry.local_entry.clone(), entry.task.clone()))
    }
    pub(crate) fn retained_states(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<Vec<ProgressItemState>> {
        self.tasks
            .get(&(*peer, source))
            .and_then(|entry| entry.retained_states.clone())
    }
    pub(crate) fn local_scope(
        &self,
        entry: &EntryId,
        attempt: ProgressIdentity,
    ) -> Option<(DeviceId, ProgressSourceTask)> {
        self.tasks
            .iter()
            .find(|(_, value)| {
                value.local_entry == *entry && value.task.descriptor.attempt == attempt
            })
            .map(|((peer, _), value)| (*peer, value.task.clone()))
    }
    pub(crate) fn retain_states(
        &mut self,
        peer: &DeviceId,
        descriptor: ProgressTaskDescriptor,
        items: Vec<ProgressItemState>,
    ) {
        if let Some(entry) = self.tasks.get_mut(&(*peer, descriptor.source)) {
            if entry.task.descriptor == descriptor && items.len() == entry.task.items.len() {
                entry.retained_states = Some(items);
            }
        }
    }
    pub(crate) fn publish_snapshot(
        &mut self,
        peer: &DeviceId,
        descriptor: ProgressTaskDescriptor,
        mut items: Vec<ProgressItemState>,
    ) -> Result<Arc<ProgressSnapshot>, ProgressReceiveError> {
        let current = self
            .tasks
            .get_mut(&(*peer, descriptor.source))
            .ok_or(ProgressReceiveError::UnknownTask)?;
        if current.task.descriptor != descriptor {
            return Err(ProgressReceiveError::Stale);
        }
        if items.len() != current.task.items.len()
            || items
                .iter()
                .zip(current.task.items.iter())
                .any(|(state, planned)| state.item() != *planned)
        {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        if let Some(previous) = &current.snapshot {
            for (next, previous) in items.iter_mut().zip(&previous.items) {
                if normalized(*next) == normalized(*previous) {
                    *next = *previous;
                } else if matches!(previous, ProgressItemState::Terminal(_)) {
                    // 迟到读取不能改写实际已结算结果；同一尝试的冲突仍明确拒绝。
                    if matches!(next, ProgressItemState::Terminal(_)) {
                        return Err(ProgressReceiveError::Conflict);
                    }
                    *next = *previous;
                } else {
                    let sequence = sequence(*previous)
                        .checked_add(1)
                        .ok_or(ProgressReceiveError::Stale)?;
                    set_sequence(next, sequence);
                }
            }
            if items == previous.items {
                return Ok(previous.clone());
            }
        } else {
            for item in &mut items {
                set_sequence(item, 1);
            }
        }
        let revision = self
            .snapshot_revision
            .checked_add(1)
            .ok_or(ProgressReceiveError::Stale)?;
        self.snapshot_revision = revision;
        let snapshot = Arc::new(ProgressSnapshot {
            descriptor,
            revision,
            items,
        });
        current.snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }
}
fn normalized(mut value: ProgressItemState) -> ProgressItemState {
    set_sequence(&mut value, 0);
    value
}
fn sequence(value: ProgressItemState) -> u64 {
    match value {
        ProgressItemState::Active(value) => value.sequence,
        ProgressItemState::Terminal(value) => value.sequence,
    }
}
fn set_sequence(value: &mut ProgressItemState, sequence: u64) {
    match value {
        ProgressItemState::Active(value) => value.sequence = sequence,
        ProgressItemState::Terminal(value) => value.sequence = sequence,
    }
}
