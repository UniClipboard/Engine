use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{warn, Instrument, Span};

#[derive(Clone)]
pub struct SearchTaskError(Vec<Arc<JoinError>>);

impl SearchTaskError {
    pub fn failures(&self) -> &[Arc<JoinError>] {
        &self.0
    }
}

impl fmt::Debug for SearchTaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchTaskError")
            .field("failure_count", &self.failures().len())
            .finish()
    }
}

impl fmt::Display for SearchTaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("search background work failed")
    }
}

impl Error for SearchTaskError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.first().map(|error| error.as_ref() as &dyn Error)
    }
}

struct State {
    cancel: CancellationToken,
    tasks: TaskTracker,
    open: bool,
    stopped: bool,
    failures: Vec<Arc<JoinError>>,
}

#[cfg(test)]
mod tests {
    use super::SearchTaskScope;
    use std::error::Error;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn pause_cannot_reopen_until_registered_work_exits() {
        let scope = SearchTaskScope::new();
        let (release, held) = oneshot::channel();
        let (cancelled, cancellation) = oneshot::channel();
        assert!(scope
            .spawn("search.test.held", |cancel| async move {
                cancel.cancelled().await;
                let _ = cancelled.send(());
                let _ = held.await;
            })
            .is_some());
        let closing = {
            let scope = scope.clone();
            tokio::spawn(async move { scope.close(false).await })
        };
        cancellation.await.unwrap();
        assert!(!scope.reopen());
        assert!(!closing.is_finished());
        assert!(scope.spawn("search.test.rejected", |_| async {}).is_none());
        release.send(()).unwrap();
        closing.await.unwrap().unwrap();
        assert!(scope.reopen());
        assert!(scope.spawn("search.test.resumed", |_| async {}).is_some());
        scope.close(true).await.unwrap();
        assert!(!scope.reopen());
        assert!(scope.is_empty());
    }

    #[tokio::test]
    async fn failures_survive_repeated_close_and_wait_for_other_work() {
        let scope = SearchTaskScope::new();
        let (release, held) = oneshot::channel();
        let (cancelled, cancellation) = oneshot::channel();
        assert!(scope
            .spawn("search.test.held", |cancel| async move {
                cancel.cancelled().await;
                cancelled.send(()).unwrap();
                held.await.unwrap();
            })
            .is_some());
        for _ in 0..2 {
            assert!(scope
                .spawn("search.test.panic", |_| async {
                    panic!("private-search-panic");
                })
                .is_some());
        }
        cancellation.await.unwrap();
        let closing = {
            let scope = scope.clone();
            tokio::spawn(async move { scope.close(false).await })
        };
        tokio::task::yield_now().await;
        assert!(!closing.is_finished());
        release.send(()).unwrap();
        let error = closing.await.unwrap().unwrap_err();
        assert_eq!(error.failures().len(), 2);
        assert!(error.failures().iter().all(|failure| failure.is_panic()));
        assert!(error.source().unwrap().is::<tokio::task::JoinError>());
        assert!(!format!("{error:?} {error}").contains("private-search-panic"));
        assert!(!scope.reopen());
        assert_eq!(scope.close(true).await.unwrap_err().failures().len(), 2);
    }

    #[tokio::test]
    async fn completed_run_has_already_recorded_its_failure() {
        let scope = SearchTaskScope::new();
        scope
            .run("search.test.startup", |_| async {
                panic!("startup failed")
            })
            .await;
        assert!(scope.result().is_err());
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
            failures: Vec::new(),
        })))
    }

    pub(super) fn reopen(&self) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped || !state.failures.is_empty() || (!state.open && !state.tasks.is_empty()) {
            return false;
        }
        if !state.open {
            state.cancel = CancellationToken::new();
            state.tasks = TaskTracker::new();
            state.open = true;
        }
        true
    }

    pub(super) async fn close(&self, permanent: bool) -> Result<(), SearchTaskError> {
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
        self.result()
    }

    pub(super) fn result(&self) -> Result<(), SearchTaskError> {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.failures.is_empty() {
            Ok(())
        } else {
            Err(SearchTaskError(state.failures.clone()))
        }
    }

    pub(super) fn spawn<F, Fut>(&self, name: &'static str, work: F) -> Option<JoinHandle<()>>
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
                return None;
            }
            (state.cancel.child_token(), state.tasks.token())
        };
        let owner = self.clone();
        Some(tokio::spawn(
            async move {
                // 取消由工作在动作边界处理，不能丢弃仍等待磁盘线程的 future。
                let result =
                    tokio::spawn(async move { work(cancel).await }.instrument(Span::current()))
                        .await;
                if let Err(source) = result {
                    owner.record_failure(source);
                    warn!(
                        event = "task.panicked",
                        task = name,
                        "search background task failed"
                    );
                }
                drop(tracker);
            }
            .instrument(Span::current()),
        ))
    }

    pub(super) async fn run<F, Fut>(&self, name: &'static str, work: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Some(task) = self.spawn(name, work) {
            if let Err(source) = task.await {
                self.record_failure(source);
            }
        }
    }

    fn record_failure(&self, source: JoinError) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.failures.push(Arc::new(source));
        state.open = false;
        state.cancel.cancel();
        state.tasks.close();
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
