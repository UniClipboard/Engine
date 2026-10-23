//! 在后续项提交期间持续读取已经交付的资源，任何瞬时缺失都使回归失败。
use crate::{Engine, HistoryEntryInput, Operation, OperationResult};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct ResourceReads {
    cancel: CancellationToken,
    task: Option<JoinHandle<usize>>,
}

impl ResourceReads {
    pub(super) fn start(engine: Arc<Engine>, entry: String) -> Self {
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let task = tokio::spawn(async move {
            let mut reads = 0;
            loop {
                let result = tokio::select! {
                    biased;
                    _ = stop.cancelled() => break,
                    result = engine.execute(Operation::GetHistoryEntryResource(HistoryEntryInput { entry_id: entry.clone() })) => result,
                };
                assert!(
                    matches!(result, Ok(OperationResult::HistoryEntryResource(_))),
                    "已交付资源在后续项提交期间必须保持可读"
                );
                reads += 1;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            reads
        });
        Self {
            cancel,
            task: Some(task),
        }
    }

    pub(super) async fn finish(mut self) {
        self.cancel.cancel();
        assert!(self.task.take().unwrap().await.unwrap() > 0);
    }
}

impl Drop for ResourceReads {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
