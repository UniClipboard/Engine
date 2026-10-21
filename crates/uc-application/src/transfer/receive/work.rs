//! 接收动作的实际寿命；只保存内存围栏和完成事实，不代替持久 attempt 权威。

use super::batch::ReceiveBatch;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::sync::watch;
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ids::EntryId;
use uc_observability_contract::wire_enum;

type Completion = Option<Result<(), Arc<ReceiveWorkError>>>;
type Key = (EntryId, String);

pub(crate) struct ReceiveWorkRegistry {
    state: Mutex<RegistryState>,
}

struct RegistryState {
    accepting: bool,
    generation: CancellationToken,
    active: HashMap<Key, Weak<Progress>>,
    cleaning: HashSet<String>,
}

impl RegistryState {
    fn owns_receive(&self, entry: &str) -> bool {
        self.active.iter().any(|((id, _), progress)| {
            id.as_ref() == entry
                && progress
                    .upgrade()
                    .is_some_and(|progress| progress.completion.borrow().is_none())
        })
    }

    fn seal(&mut self) {
        self.accepting = false;
        self.generation.cancel();
        self.generation = CancellationToken::new();
    }
}

impl Default for ReceiveWorkRegistry {
    fn default() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                accepting: true,
                generation: CancellationToken::new(),
                active: HashMap::new(),
                cleaning: HashSet::new(),
            }),
        }
    }
}

struct Progress {
    cancel: CancellationToken,
    completion: watch::Sender<Completion>,
}

/// 同一接收处理和其监督者共同持有；只有监督者能声明整个动作结束。
pub(crate) struct ReceiveAction {
    pub(crate) batch: Arc<ReceiveBatch>,
    registry: Arc<ReceiveWorkRegistry>,
    progress: Mutex<Option<Arc<Progress>>>,
    generation: CancellationToken,
}

/// 旧行清理与新接收登记互斥；跨异步清理持有，退出时才释放。
pub(crate) struct ReceiveCleanup {
    registry: Arc<ReceiveWorkRegistry>,
    entry: String,
}

impl Drop for ReceiveCleanup {
    fn drop(&mut self) {
        self.registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cleaning
            .remove(&self.entry);
    }
}

wire_enum! {
    #[derive(Debug, Clone, Copy, thiserror::Error)]
    pub(crate) enum ReceiveRegistrationError: &'static str {
        #[error("receive gate generation is closed")]
        GateClosed => "receive_gate_closed",
        #[error("receive cleanup is still in progress")]
        CleanupInProgress => "receive_cleanup_in_progress",
    }
}

impl ErrorClass for ReceiveRegistrationError {
    fn class(&self) -> &'static str {
        self.wire()
    }
}

pub(crate) struct StoppedReceive {
    completion: watch::Receiver<Completion>,
}

impl ReceiveWorkRegistry {
    pub(crate) fn action(self: &Arc<Self>) -> Arc<ReceiveAction> {
        Arc::new(ReceiveAction {
            batch: Arc::new(ReceiveBatch::default()),
            registry: self.clone(),
            progress: Mutex::new(None),
            generation: self.generation(),
        })
    }

    pub(crate) fn generation(&self) -> CancellationToken {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
            .clone()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accepting
    }

    pub(crate) fn open(&self, generation: &CancellationToken) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if generation.is_cancelled() {
            return false;
        }
        state.accepting = true;
        true
    }

    /// 封住旧动作；保留尚未结算的工作登记，扫描和取消仍须等待它们。
    pub(crate) fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.seal();
    }

    pub(crate) fn close_if_current(&self, generation: &CancellationToken) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if generation.is_cancelled() {
            return false;
        }
        state.seal();
        true
    }

    pub(crate) fn owns_entry(&self, entry: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.cleaning.contains(entry) || state.owns_receive(entry)
    }

    pub(crate) fn claim_cleanup(self: &Arc<Self>, entry: &str) -> Option<ReceiveCleanup> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.owns_receive(entry) || !state.cleaning.insert(entry.to_owned()) {
            return None;
        }
        Some(ReceiveCleanup {
            registry: self.clone(),
            entry: entry.to_owned(),
        })
    }

    pub(crate) fn stop(&self, entry: &EntryId, attempt: &str) -> Option<StoppedReceive> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let progress = state
            .active
            .get(&(entry.clone(), attempt.to_owned()))?
            .upgrade()?;
        progress.cancel.cancel();
        Some(StoppedReceive {
            completion: progress.completion.subscribe(),
        })
    }
}

impl ReceiveAction {
    pub(crate) fn is_registered(&self) -> bool {
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// 必须先登记，再写入可被外部取消的 attempt。
    pub(crate) fn register(
        &self,
        entry: &EntryId,
        attempt: &str,
    ) -> Result<(), ReceiveRegistrationError> {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || self.generation.is_cancelled() {
            return Err(ReceiveRegistrationError::GateClosed);
        }
        if state.cleaning.contains(entry.as_ref()) {
            return Err(ReceiveRegistrationError::CleanupInProgress);
        }
        let progress = Arc::new(Progress {
            cancel: self.generation.child_token(),
            completion: watch::channel(None).0,
        });
        state.active.retain(|_, value| value.strong_count() != 0);
        state.active.insert(
            (entry.clone(), attempt.to_owned()),
            Arc::downgrade(&progress),
        );
        *self.progress.lock().unwrap_or_else(PoisonError::into_inner) = Some(progress);
        Ok(())
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|progress| progress.cancel.clone())
            .unwrap_or_else(|| self.generation.child_token())
    }

    pub(crate) fn finish(&self, result: Result<(), ReceiveWorkError>) {
        self.batch.finish(result.is_ok());
        if let Some(progress) = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            progress
                .completion
                .send_replace(Some(result.map_err(Arc::new)));
        }
    }
}

impl StoppedReceive {
    pub(crate) async fn wait(mut self) -> Result<(), Arc<ReceiveWorkError>> {
        loop {
            if let Some(result) = self.completion.borrow_and_update().clone() {
                return result;
            }
            self.completion
                .changed()
                .await
                .map_err(|source| Arc::new(ReceiveWorkError::Completion(source)))?;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReceiveWorkError {
    #[error("receive action stopped before settlement")]
    Stopped,
    #[error("receive action failed during settlement")]
    Action(#[source] anyhow::Error),
    #[error("receive task failed before settlement")]
    Task(#[source] Arc<JoinError>),
    #[error("receive settlement completion was lost")]
    Completion(#[source] watch::error::RecvError),
}

#[cfg(test)]
mod tests;
