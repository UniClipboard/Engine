use std::sync::{Arc, Mutex};

use uc_core::search::{RebuildProgress, SearchIndexMeta};

use super::coordinator::{
    REASON_INTERRUPTED_REBUILD, REASON_REBUILD_FAILED_WAITING, STATUS_READY, STATUS_REBUILDING,
    STATUS_UNAVAILABLE,
};
use super::{SearchRebuildProgressView, SearchStatusView};

/// 产品状态通知出口；实现必须立即返回，不得回调搜索流程或等待消费者。
pub trait SearchStatusEventPort: Send + Sync {
    fn changed(&self, status: SearchStatusView);
}

/// 查询和通知共用一份状态，修改和发布保持同序，不保留无人接收的第二条通知通道。
pub(super) struct SearchStatusStore {
    current: Mutex<SearchStatusView>,
    events: Arc<dyn SearchStatusEventPort>,
}

impl SearchStatusStore {
    pub fn new(events: Arc<dyn SearchStatusEventPort>) -> Self {
        Self {
            current: Mutex::new(SearchStatusView {
                state: STATUS_UNAVAILABLE.to_owned(),
                reason: None,
                progress: None,
                last_rebuild_started_at_ms: None,
                last_rebuild_completed_at_ms: None,
            }),
            events,
        }
    }

    pub fn snapshot(&self) -> SearchStatusView {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn update(&self, change: impl FnOnce(&mut SearchStatusView)) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let before = current.clone();
        change(&mut current);
        if *current != before {
            self.events.changed(current.clone());
        }
    }

    pub fn restore_metadata(&self, meta: &SearchIndexMeta) {
        self.update(|state| {
            state.last_rebuild_started_at_ms = meta.last_rebuild_started_at_ms;
            state.last_rebuild_completed_at_ms = meta.last_rebuild_completed_at_ms;
        });
    }

    pub fn set_status(&self, status: &str, reason: Option<&str>) {
        self.update(|state| {
            state.state = status.to_owned();
            state.reason = reason.map(str::to_owned);
            if status == STATUS_READY
                && state
                    .progress
                    .as_ref()
                    .is_some_and(|p| p.stage != "complete")
            {
                state.progress = None;
            }
        });
    }

    pub fn begin(self: &Arc<Self>, reason: &str) -> RebuildStatusGuard {
        self.update(|state| {
            state.state = STATUS_REBUILDING.to_owned();
            state.reason = Some(reason.to_owned());
            state.last_rebuild_started_at_ms = Some(chrono::Utc::now().timestamp_millis());
            state.progress = Some(SearchRebuildProgressView {
                stage: "preparing".to_owned(),
                indexed: 0,
                total: None,
            });
        });
        RebuildStatusGuard(Arc::clone(self))
    }

    pub fn progress(&self, progress: RebuildProgress) {
        self.update(|state| {
            // 下层完成事件不提前宣布整个流程成功，最终结果由协调器结算。
            state.progress = Some(SearchRebuildProgressView {
                stage: "indexing".to_owned(),
                indexed: progress.indexed,
                total: Some(progress.total),
            });
        });
    }

    pub fn preparing(&self, processed: usize) {
        self.update(|state| {
            if let Some(progress) = &mut state.progress {
                progress.indexed = processed.min(u32::MAX as usize) as u32;
            }
        });
    }

    pub fn complete(&self) {
        self.update(|state| {
            state.state = STATUS_READY.to_owned();
            state.reason = None;
            state.last_rebuild_completed_at_ms = Some(chrono::Utc::now().timestamp_millis());
            if let Some(progress) = &mut state.progress {
                progress.stage = "complete".to_owned();
            }
        });
    }

    pub fn fail(&self) {
        self.update(|state| {
            state.state = STATUS_UNAVAILABLE.to_owned();
            state.reason = Some(REASON_REBUILD_FAILED_WAITING.to_owned());
            if let Some(progress) = &mut state.progress {
                progress.stage = "failed".to_owned();
            }
        });
    }

    pub fn pause(&self) {
        self.update(|state| {
            if state.state != STATUS_UNAVAILABLE {
                state.state = STATUS_UNAVAILABLE.to_owned();
                state.reason = Some("session_paused".to_owned());
            }
        });
    }
}

/// 异步任务被取消或发生 panic 时，也必须留下可查询的终态。
pub(super) struct RebuildStatusGuard(Arc<SearchStatusStore>);

impl Drop for RebuildStatusGuard {
    fn drop(&mut self) {
        self.0.update(|state| {
            if state.state == STATUS_REBUILDING {
                let panicked = std::thread::panicking();
                state.state = STATUS_UNAVAILABLE.to_owned();
                state.reason = Some(
                    if panicked {
                        REASON_REBUILD_FAILED_WAITING
                    } else {
                        REASON_INTERRUPTED_REBUILD
                    }
                    .to_owned(),
                );
                if let Some(progress) = &mut state.progress {
                    progress.stage = if panicked { "failed" } else { "cancelled" }.to_owned();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Events(Mutex<Vec<SearchStatusView>>);

    impl SearchStatusEventPort for Events {
        fn changed(&self, status: SearchStatusView) {
            self.0.lock().unwrap().push(status);
        }
    }

    #[test]
    fn preparing_has_unknown_total_and_restart_clears_previous_terminal() {
        let events = Arc::new(Events::default());
        let store = Arc::new(SearchStatusStore::new(events.clone()));
        {
            let _guard = store.begin("manual_rebuild");
            store.preparing(200);
            assert_eq!(store.snapshot().progress.unwrap().total, None);
            store.fail();
        }
        let _guard = store.begin("manual_rebuild");
        let snapshot = store.snapshot();
        let progress = snapshot.progress.as_ref().unwrap();
        assert_eq!(progress.stage, "preparing");
        assert_eq!(progress.indexed, 0);
        assert_eq!(events.0.lock().unwrap().last(), Some(&snapshot));
    }

    #[test]
    fn unwinding_records_failure_instead_of_leaving_rebuild_running() {
        let store = Arc::new(SearchStatusStore::new(Arc::new(Events::default())));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = store.begin("manual_rebuild");
            panic!("injected rebuild failure");
        }));
        assert!(result.is_err());
        let snapshot = store.snapshot();
        assert_eq!(snapshot.state, STATUS_UNAVAILABLE);
        assert_eq!(snapshot.progress.unwrap().stage, "failed");
    }
}
