//! 在原接收目录锁内读取计划、实际保存事实和当前内容修订；不另建结果账本。
use super::super::RegistryState;
use super::{ReceiveAction, ReceiveWorkRegistry};
use crate::transfer::progress::{
    ProgressPageReply, ProgressReceiveError, ProgressSnapshotPage, ProgressSnapshotProviderPort,
    ProgressSnapshotQuery, ProgressSourceTask,
};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::{atomic::Ordering, PoisonError};
use uc_core::file_transfer::{ProgressIdentity, ProgressItemState, MAX_SNAPSHOT_PAGE_ITEMS};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

#[async_trait]
impl ProgressSnapshotProviderPort for ReceiveWorkRegistry {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.clearing || state.clear_failed {
            return Ok(ProgressPageReply::AccessDenied);
        }
        let Some((entry, current)) = state.progress_catalog.query_scope(peer, query.source) else {
            return Ok(ProgressPageReply::NotRetained);
        };
        let items = state
            .progress_states(&entry, &current)
            .or_else(|| state.progress_catalog.retained_states(peer, query.source))
            .ok_or(ProgressReceiveError::UnknownTask)?;
        let snapshot = state
            .progress_catalog
            .publish_snapshot(peer, current.descriptor, items)?;
        if query
            .revision
            .is_some_and(|revision| revision != snapshot.revision)
        {
            return Err(ProgressReceiveError::Stale);
        }
        let offset = usize::from(query.offset);
        if offset >= snapshot.items.len() || (offset > 0 && query.revision.is_none()) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        let page = ProgressSnapshotPage {
            descriptor: snapshot.descriptor,
            revision: snapshot.revision,
            offset: query.offset,
            total: snapshot.descriptor.item_count,
            items: snapshot.items
                [offset..(offset + MAX_SNAPSHOT_PAGE_ITEMS).min(snapshot.items.len())]
                .to_vec(),
        };
        Ok(
            if query
                .attempt
                .is_some_and(|attempt| attempt != snapshot.descriptor.attempt)
            {
                ProgressPageReply::Superseded(page)
            } else {
                ProgressPageReply::Current(page)
            },
        )
    }
}

impl RegistryState {
    pub(in super::super) fn progress_states(
        &self,
        entry: &EntryId,
        current: &ProgressSourceTask,
    ) -> Option<Vec<ProgressItemState>> {
        if let Some(receipt) = self.receipts.get(entry).filter(|receipt| {
            Uuid::parse_str(&receipt.attempt)
                .is_ok_and(|attempt| attempt.as_bytes() == &current.descriptor.attempt.bytes())
        }) {
            let committed = receipt.landed.keys().copied().collect::<HashSet<_>>();
            let facts = receipt.batch.upgrade().map(|batch| batch.facts());
            return facts
                .as_ref()
                .or(receipt.final_facts.as_ref())?
                .progress_states(&receipt.planned, &current.items, &committed, false);
        }
        let active = self
            .active
            .iter()
            .find(|((id, attempt), _)| {
                id == entry
                    && Uuid::parse_str(attempt).is_ok_and(|attempt| {
                        attempt.as_bytes() == &current.descriptor.attempt.bytes()
                    })
            })?
            .1
            .upgrade()?;
        let planned = active
            .prepared_plan
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        active.batch.upgrade()?.facts().progress_states(
            &planned,
            &current.items,
            &HashSet::new(),
            active.fully_persisted.load(Ordering::Acquire),
        )
    }
}
impl ReceiveAction {
    pub(in super::super) fn retain_final_progress(&self) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(progress) = progress else {
            return;
        };
        let Ok(attempt) = Uuid::parse_str(&progress.attempt) else {
            return;
        };
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some((peer, current)) = state
            .progress_catalog
            .local_scope(&progress.entry, ProgressIdentity::new(*attempt.as_bytes()))
        else {
            return;
        };
        if let Some(items) = state.progress_states(&progress.entry, &current) {
            state
                .progress_catalog
                .retain_states(&peer, current.descriptor, items);
        }
    }
}
