//! 接收动作的实际寿命；只保存内存围栏和完成事实，不代替持久 attempt 权威。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::sync::watch;
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;
use uc_core::ids::EntryId;

type Completion = Option<Result<(), Arc<ReceiveWorkError>>>;
type Key = (EntryId, String);

#[derive(Default)]
pub(crate) struct ReceiveWorkRegistry {
    active: Mutex<HashMap<Key, Weak<Progress>>>,
}

struct Progress {
    cancel: CancellationToken,
    completion: watch::Sender<Completion>,
}

/// 同一接收处理和其监督者共同持有；只有监督者能声明整个动作结束。
pub(crate) struct ReceiveAction {
    registry: Arc<ReceiveWorkRegistry>,
    progress: Mutex<Option<Arc<Progress>>>,
}

pub(crate) struct StoppedReceive {
    completion: watch::Receiver<Completion>,
}

impl ReceiveWorkRegistry {
    pub(crate) fn action(self: &Arc<Self>) -> Arc<ReceiveAction> {
        Arc::new(ReceiveAction {
            registry: self.clone(),
            progress: Mutex::new(None),
        })
    }

    pub(crate) fn stop(&self, entry: &EntryId, attempt: &str) -> Option<StoppedReceive> {
        let active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        let progress = active
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
    pub(crate) fn register(&self, entry: &EntryId, attempt: &str) {
        let progress = Arc::new(Progress {
            cancel: CancellationToken::new(),
            completion: watch::channel(None).0,
        });
        let mut active = self
            .registry
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        active.retain(|_, value| value.strong_count() != 0);
        active.insert(
            (entry.clone(), attempt.to_owned()),
            Arc::downgrade(&progress),
        );
        *self.progress.lock().unwrap_or_else(PoisonError::into_inner) = Some(progress);
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|progress| progress.cancel.clone())
            .unwrap_or_default()
    }

    pub(crate) fn finish(&self, result: Result<(), ReceiveWorkError>) {
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
