//! 原投影负责人完成快照采用；网络等待在锁外，保存和发布仍受当前意图围栏保护。
use super::{
    fresh_notification, ProgressFreshnessInput, ProgressFreshnessView, State,
    TransferProgressReceiver,
};
use crate::transfer::progress::{
    ProgressReceiveError, ProgressSnapshot, ProgressSourceTask, ProgressTaskDescriptor,
};
use std::sync::{Arc, PoisonError};
use uc_core::file_transfer::{
    merge_progress_snapshot, ProgressIdentity, ProgressItemProjection, ProgressItemState,
    ProgressSnapshotMerge, ProgressSnapshotRefusal,
};

impl TransferProgressReceiver {
    pub(in super::super) fn query_scope(
        &self,
    ) -> Result<(ProgressIdentity, ProgressSourceTask), ProgressReceiveError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let bound = state.bound.ok_or(ProgressReceiveError::UnknownTask)?;
        Ok((
            self.query_intent,
            ProgressSourceTask {
                descriptor: bound.descriptor,
                items: Arc::from(state.plan.clone()),
                notification_closed: Default::default(),
            },
        ))
    }
    pub(in super::super) fn adopt_snapshot(
        &self,
        expected: ProgressTaskDescriptor,
        snapshot: &ProgressSnapshot,
    ) -> Result<(), ProgressReceiveError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed
            || state.bound.is_none_or(|bound| bound.descriptor != expected)
            || snapshot.descriptor != expected
            || state
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
