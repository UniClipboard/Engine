//! 原投影负责人完成快照采用；查询可确认初始任务，但不能伪造通知绑定。
use super::{
    fresh_notification, ProgressFreshnessInput, ProgressFreshnessView, State,
    TransferProgressReceiver,
};
use crate::transfer::progress::{
    ProgressReceiveError, ProgressSnapshot, ProgressSnapshotTarget, ProgressTaskDescriptor,
};
use std::sync::{Arc, PoisonError};
use uc_core::file_transfer::{
    merge_progress_snapshot, ProgressIdentity, ProgressItemProjection, ProgressItemState,
    ProgressSnapshotMerge, ProgressSnapshotRefusal, RegistrationEffect, RegistrationInput,
};

impl State {
    pub(super) fn task_descriptor(&self) -> Option<ProgressTaskDescriptor> {
        self.bound
            .map(|bound| bound.descriptor)
            .or(self.queried_scope)
    }
}
impl TransferProgressReceiver {
    pub(in super::super) fn query_scope(
        &self,
    ) -> Result<(ProgressIdentity, ProgressSnapshotTarget), ProgressReceiveError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        Ok((
            self.query_intent,
            ProgressSnapshotTarget {
                source: self.source,
                expected: state.task_descriptor(),
                items: Arc::from(state.plan.clone()),
            },
        ))
    }
    pub(in super::super) fn adopt_snapshot(
        &self,
        expected: Option<ProgressTaskDescriptor>,
        snapshot: &ProgressSnapshot,
    ) -> Result<(), ProgressReceiveError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let current = state.task_descriptor();
        if state.closed
            || expected.is_some_and(|expected| {
                current != Some(expected) || snapshot.descriptor != expected
            })
            || (expected.is_none() && current.is_some_and(|current| current != snapshot.descriptor))
        {
            return Err(ProgressReceiveError::Stale);
        }
        if snapshot.descriptor.source != self.source
            || usize::from(snapshot.descriptor.item_count) != state.plan.len()
        {
            return Err(ProgressSnapshotRefusal::DescriptorChanged.into());
        }
        if state
            .snapshot_revision
            .is_some_and(|revision| snapshot.revision < revision)
        {
            return Err(ProgressReceiveError::Stale);
        }
        if state.snapshot_revision == Some(snapshot.revision)
            && state
                .snapshot_content
                .as_ref()
                .is_some_and(|old| old != &snapshot.items)
        {
            return Err(ProgressSnapshotRefusal::StateConflict.into());
        }
        let old = state
            .plan
            .iter()
            .map(|item| {
                state
                    .items
                    .get(item)
                    .and_then(ProgressItemProjection::state)
            })
            .collect::<Vec<_>>();
        let merged = merge_progress_snapshot(&state.plan, &old, &snapshot.items)?;
        if state.registration.apply(RegistrationInput::QueryVerified {
            descriptor: snapshot.descriptor,
        }) != RegistrationEffect::QueryVerified
        {
            return Err(ProgressReceiveError::Stale);
        }
        if state.bound.is_none() {
            state.queried_scope = Some(snapshot.descriptor);
        }
        state.snapshot_revision = Some(snapshot.revision);
        state.snapshot_content = Some(snapshot.items.clone());
        if let ProgressSnapshotMerge::Adopt(items) = merged {
            state.items = state.plan.iter().copied().zip(items).collect();
            fresh_notification(&self.clock, &mut state);
            settle(&mut state);
            state.version = state.version.saturating_add(1);
        }
        Ok(())
    }
}
fn settle(state: &mut State) {
    if state.items.len() == state.plan.len()
        && state
            .items
            .values()
            .all(|item| matches!(item.state(), Some(ProgressItemState::Terminal(_))))
    {
        state.freshness.apply(ProgressFreshnessInput::Settled);
        state.view = ProgressFreshnessView::Settled;
    }
}
