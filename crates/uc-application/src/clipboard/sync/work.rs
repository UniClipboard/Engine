use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;
use tokio::task::JoinError;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};

use crate::runtime_lifecycle::LifecycleError;

#[cfg(test)]
mod tests;

#[derive(Clone, Default)]
pub(super) struct WorkOwner(Arc<Shared>);

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Default)]
struct State {
    closed: bool,
    active: usize,
    failures: Vec<Arc<JoinError>>,
}

pub(super) struct OwnedWork(Arc<Shared>);

impl WorkOwner {
    pub(super) fn begin(&self) -> Option<OwnedWork> {
        let mut state = self.0.state();
        if state.closed {
            return None;
        }
        state.active += 1;
        Some(OwnedWork(Arc::clone(&self.0)))
    }

    pub(super) async fn shutdown(&self) -> Result<(), LifecycleError> {
        self.0.state().closed = true;
        self.0.changed.notify_waiters();
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.0.state();
                if state.active == 0 {
                    return LifecycleError::from_errors(
                        state
                            .failures
                            .iter()
                            .cloned()
                            .map(anyhow::Error::new)
                            .collect(),
                    );
                }
            }
            changed.await;
        }
    }
}

impl OwnedWork {
    pub(super) async fn stopped(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.0.state().closed {
                return;
            }
            changed.await;
        }
    }

    pub(super) fn spawn(
        self,
        kind: DiagnosticTaskKind,
        future: impl Future<Output = ()> + Send + 'static,
    ) {
        let worker = tokio::spawn(future);
        tokio::spawn(async move {
            if let Err(source) = worker.await {
                self.failed(source);
                record_task_join_failure(kind);
            }
            drop(self);
        });
    }

    pub(super) fn continuation(&self) -> Self {
        // 已接收动作可以转交自己的后续工作；原许可保留到前台记录完成。
        self.0.state().active += 1;
        Self(Arc::clone(&self.0))
    }

    pub(super) fn failed(&self, source: JoinError) -> Arc<JoinError> {
        let source = Arc::new(source);
        let mut state = self.0.state();
        state.closed = true;
        state.failures.push(Arc::clone(&source));
        self.0.changed.notify_waiters();
        source
    }
}

impl Drop for OwnedWork {
    fn drop(&mut self) {
        self.0.state().active -= 1;
        self.0.changed.notify_waiters();
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
