//! 原接收目录提供已接受任务的完整变化事实；不维护第二套结果或通知序号。
use super::ReceiveWorkRegistry;
use crate::transfer::progress::{ProgressReceiveError, ProgressSnapshot};
use std::sync::{Arc, PoisonError};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

pub(crate) struct ReceiveProgressChange {
    pub peer: DeviceId,
    pub snapshot: Result<Arc<ProgressSnapshot>, ProgressReceiveError>,
}
impl ReceiveWorkRegistry {
    pub(crate) async fn next_progress_changes(
        &self,
        cancel: &CancellationToken,
    ) -> Option<Vec<ReceiveProgressChange>> {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if cancel.is_cancelled() {
                return None;
            }
            let changed = self.take_progress_changes();
            if !changed.is_empty() {
                return Some(changed);
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                _ = &mut notified => {},
            }
        }
    }
    fn take_progress_changes(&self) -> Vec<ReceiveProgressChange> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.clearing || state.clear_failed {
            return Vec::new();
        }
        state
            .progress_catalog
            .take_changed()
            .into_iter()
            .map(|(peer, entry, current)| {
                let snapshot = state
                    .progress_states(&entry, &current)
                    .or_else(|| {
                        state
                            .progress_catalog
                            .retained_states(&peer, current.descriptor.source)
                    })
                    .ok_or(ProgressReceiveError::UnknownTask)
                    .and_then(|items| {
                        state
                            .progress_catalog
                            .publish_snapshot(&peer, current.descriptor, items)
                    });
                ReceiveProgressChange { peer, snapshot }
            })
            .collect()
    }
}
