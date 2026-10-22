//! 接收运行世代、实际寿命与同进程来源；持久 attempt 仍是权威。

use super::batch::ReceiveBatch;
use anyhow::Error as SourceError;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::sync::{watch, Mutex as AsyncMutex, Notify};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ids::EntryId;
use uc_observability_contract::wire_enum;

mod catalog;
mod clear;
mod settlement;
pub(crate) use catalog::{PlannedReceiveItem, ReceiveRepresentation, ReceiveSource};
pub(crate) use settlement::UnsettledFile;

type Completion = Option<Result<(), Arc<ReceiveWorkError>>>;
type Key = (EntryId, String);

pub(crate) struct ReceiveWorkRegistry {
    state: Mutex<RegistryState>,
    maintenance: Arc<AsyncMutex<()>>,
    pub(super) changed: Notify,
}

struct RegistryState {
    accepting: bool,
    clearing: bool,
    clear_failed: bool,
    generation: CancellationToken,
    active: HashMap<Key, Weak<Progress>>,
    cleaning: HashSet<String>,
    receipts: HashMap<EntryId, catalog::ReceiveReceipt>,
    born: Vec<Weak<Birth>>,
    retired: HashMap<EntryId, Vec<Weak<Birth>>>,
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
        self.clear_failed = false;
        self.generation.cancel();
        self.generation = CancellationToken::new();
        self.receipts.clear();
        self.retired.clear();
    }
}

impl Default for ReceiveWorkRegistry {
    fn default() -> Self {
        Self {
            maintenance: Arc::new(AsyncMutex::new(())),
            changed: Notify::new(),
            state: Mutex::new(RegistryState {
                accepting: true,
                clearing: false,
                clear_failed: false,
                generation: CancellationToken::new(),
                active: HashMap::new(),
                cleaning: HashSet::new(),
                receipts: HashMap::new(),
                born: Vec::new(),
                retired: HashMap::new(),
            }),
        }
    }
}

struct Birth {
    cancel: CancellationToken,
    registered: AtomicBool,
    completion: watch::Sender<Completion>,
}

struct Progress {
    entry: EntryId,
    attempt: String,
    cancel: CancellationToken,
    completion: watch::Sender<Completion>,
}

/// 同一接收处理和其监督者共同持有；只有监督者能声明整个动作结束。
pub(crate) struct ReceiveAction {
    pub(crate) batch: Arc<ReceiveBatch>,
    registry: Arc<ReceiveWorkRegistry>,
    progress: Mutex<Option<Arc<Progress>>>,
    previous_receipt: Mutex<Option<catalog::ReceiveReceipt>>,
    file_settlement: Mutex<Option<UnsettledFile>>,
    generation: CancellationToken,
    birth: Arc<Birth>,
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
        #[error("receive entry was deleted after this action started")]
        EntryDeleted => "receive_entry_deleted",
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
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let generation = state.generation.child_token();
        if state.clearing || state.clear_failed {
            generation.cancel();
        }
        let birth = Arc::new(Birth {
            cancel: generation.clone(),
            registered: AtomicBool::new(false),
            completion: watch::channel(None).0,
        });
        state.born.retain(|birth| birth.strong_count() != 0);
        state.born.push(Arc::downgrade(&birth));
        state.retired.retain(|_, births| {
            births.retain(|birth| birth.strong_count() != 0);
            !births.is_empty()
        });
        Arc::new(ReceiveAction {
            batch: Arc::new(ReceiveBatch::default()),
            registry: self.clone(),
            progress: Mutex::new(None),
            previous_receipt: Mutex::new(None),
            file_settlement: Mutex::new(None),
            generation,
            birth,
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
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.accepting && !state.clearing && !state.clear_failed
    }

    pub(crate) fn open(&self, generation: &CancellationToken) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if generation.is_cancelled() {
            return false;
        }
        state.accepting = true;
        state.clear_failed = false;
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
        state.clearing || state.cleaning.contains(entry) || state.owns_receive(entry)
    }

    pub(crate) fn claim_cleanup(self: &Arc<Self>, entry: &str) -> Option<ReceiveCleanup> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.clearing || state.owns_receive(entry) || !state.cleaning.insert(entry.to_owned()) {
            return None;
        }
        Some(ReceiveCleanup {
            registry: self.clone(),
            entry: entry.to_owned(),
        })
    }

    /// 删除先封住全部旧动作并发停止，再等待实际结果；不借用下载许可。
    pub(crate) async fn delete_scope(
        self: &Arc<Self>,
        entry: &EntryId,
    ) -> anyhow::Result<ReceiveCleanup> {
        let (guard, waiting) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.cleaning.insert(entry.to_string()) {
                return Err(ReceiveRegistrationError::CleanupInProgress.into());
            }
            let births = state.born.clone();
            state.retired.insert(entry.clone(), births);
            state.receipts.remove(entry);
            let waiting = state
                .active
                .iter()
                .filter(|((id, _), _)| id == entry)
                .filter_map(|(_, progress)| progress.upgrade())
                .filter_map(|progress| {
                    if progress.completion.borrow().is_some() {
                        return None;
                    }
                    progress.cancel.cancel();
                    Some(StoppedReceive {
                        completion: progress.completion.subscribe(),
                    })
                })
                .collect::<Vec<_>>();
            (
                ReceiveCleanup {
                    registry: self.clone(),
                    entry: entry.to_string(),
                },
                waiting,
            )
        };
        for waiting in waiting {
            if let Err(source) = waiting.wait().await {
                if !matches!(source.as_ref(), ReceiveWorkError::Stopped) {
                    return Err(
                        anyhow::Error::new(source).context("stop receive before entry deletion")
                    );
                }
            }
        }
        Ok(guard)
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
        if !state.accepting
            || state.clearing
            || state.clear_failed
            || self.generation.is_cancelled()
        {
            return Err(ReceiveRegistrationError::GateClosed);
        }
        if state.retired.get(entry).is_some_and(|births| {
            births
                .iter()
                .filter_map(Weak::upgrade)
                .any(|birth| Arc::ptr_eq(&birth, &self.birth))
        }) {
            return Err(ReceiveRegistrationError::EntryDeleted);
        }
        if state.cleaning.contains(entry.as_ref()) {
            return Err(ReceiveRegistrationError::CleanupInProgress);
        }
        self.birth.registered.store(true, Ordering::Release);
        let progress = Arc::new(Progress {
            entry: entry.clone(),
            attempt: attempt.to_owned(),
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
        self.finish_shared(result.map_err(Arc::new));
    }

    pub(crate) fn finish_shared(&self, result: Result<(), Arc<ReceiveWorkError>>) {
        if let Err(source) = &result {
            self.batch
                .fail_unsettled_files(Arc::new(SourceError::new(source.clone())));
        }
        self.batch.finish(result.is_ok());
        self.finish_receipt();
        self.birth.completion.send_replace(Some(result.clone()));
        if let Some(progress) = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            progress.completion.send_replace(Some(result));
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
