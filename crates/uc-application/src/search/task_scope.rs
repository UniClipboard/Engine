use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::warn;

struct State {
    cancel: CancellationToken,
    tasks: TaskTracker,
    open: bool,
    stopped: bool,
}

#[cfg(test)]
mod tests {
    use super::SearchTaskScope;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn pause_cannot_reopen_until_registered_work_exits() {
        let scope = SearchTaskScope::new();
        let (release, held) = oneshot::channel();
        let (cancelled, cancellation) = oneshot::channel();
        assert!(scope.spawn("search.test.held", |cancel| async move {
            cancel.cancelled().await;
            let _ = cancelled.send(());
            let _ = held.await;
        }));
        let closing = {
            let scope = scope.clone();
            tokio::spawn(async move { scope.close(false).await })
        };
        cancellation.await.unwrap();
        assert!(!scope.reopen());
        assert!(!closing.is_finished());
        assert!(!scope.spawn("search.test.rejected", |_| async {}));
        release.send(()).unwrap();
        closing.await.unwrap();
        assert!(scope.reopen());
        assert!(scope.spawn("search.test.resumed", |_| async {}));
        scope.close(true).await;
        assert!(!scope.reopen());
        assert!(scope.is_empty());
    }
}

#[derive(Clone)]
pub(super) struct SearchTaskScope(Arc<Mutex<State>>);

impl SearchTaskScope {
    pub(super) fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
            open: true,
            stopped: false,
        })))
    }

    pub(super) fn reopen(&self) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped || (!state.open && !state.tasks.is_empty()) {
            return false;
        }
        if !state.open {
            state.cancel = CancellationToken::new();
            state.tasks = TaskTracker::new();
            state.open = true;
        }
        true
    }

    pub(super) async fn close(&self, permanent: bool) {
        let tasks = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.stopped |= permanent;
            state.open = false;
            state.cancel.cancel();
            state.tasks.close();
            state.tasks.clone()
        };
        tasks.wait().await;
    }

    pub(super) fn spawn<F, Fut>(&self, name: &'static str, work: F) -> bool
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (cancel, tracker) = {
            let state = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.open {
                return false;
            }
            (state.cancel.child_token(), state.tasks.token())
        };
        tokio::spawn(async move {
            // 取消由工作在动作边界处理，不能丢弃仍等待磁盘线程的 future。
            let result = AssertUnwindSafe(async move { work(cancel).await })
                .catch_unwind()
                .await;
            drop(tracker);
            if let Err(error) = result {
                warn!(event = "task.panicked", task = name, error = ?error, "search background task panicked");
            }
        });
        true
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tasks
            .is_empty()
    }
}
