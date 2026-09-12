use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;
use tokio::task::JoinError;

use super::DispatchSyncError;
use crate::runtime_lifecycle::LifecycleError;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct DispatchWorkOwner(Arc<Shared>);

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

pub(super) struct DispatchWork(Arc<Shared>);

impl DispatchWorkOwner {
    pub(super) fn begin(&self) -> Result<DispatchWork, DispatchSyncError> {
        let mut state = self.0.state();
        if state.closed {
            return Err(DispatchSyncError::Stopped);
        }
        state.active += 1;
        Ok(DispatchWork(Arc::clone(&self.0)))
    }

    pub(super) async fn shutdown(&self) -> Result<(), LifecycleError> {
        self.0.state().closed = true;
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

impl DispatchWork {
    pub(super) fn continuation(&self) -> Self {
        // 已接收动作可以转交自己的后续工作；原许可保留到前台记录完成。
        self.0.state().active += 1;
        Self(Arc::clone(&self.0))
    }

    pub(super) fn failed(&self, source: JoinError) {
        let mut state = self.0.state();
        state.closed = true;
        state.failures.push(Arc::new(source));
    }
}

impl Drop for DispatchWork {
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
