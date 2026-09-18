use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub(crate) struct RegisteredOperation {
    pub(crate) id: String,
    pub(crate) cancellation: CancellationToken,
    state: Arc<InFlightOperationState>,
}

pub(crate) struct InFlightOperations {
    state: Arc<InFlightOperationState>,
    next_id: AtomicU64,
}

struct InFlightOperationState {
    operations: Mutex<HashMap<String, OperationState>>,
    changed: Notify,
}

struct OperationState {
    cancellation: CancellationToken,
    terminal_reported: bool,
}

impl InFlightOperations {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(InFlightOperationState {
                operations: Mutex::new(HashMap::new()),
                changed: Notify::new(),
            }),
            next_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn register(&self, prefix: &str) -> RegisteredOperation {
        let id = format!("{prefix}-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let cancellation = CancellationToken::new();
        self.state
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                id.clone(),
                OperationState {
                    cancellation: cancellation.clone(),
                    terminal_reported: false,
                },
            );
        RegisteredOperation {
            id,
            cancellation,
            state: Arc::clone(&self.state),
        }
    }

    pub(crate) async fn finish(&self, operation_id: &str) -> bool {
        let removed = self
            .state
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(operation_id);
        if removed.is_some() {
            self.state.changed.notify_one();
        }
        removed.is_some_and(|operation| !operation.terminal_reported)
    }

    pub(crate) async fn wait_until_empty(&self, deadline: Duration) -> bool {
        tokio::time::timeout(deadline, self.wait_empty())
            .await
            .is_ok()
    }

    pub(super) async fn wait_empty(&self) {
        loop {
            let changed = self.state.changed.notified();
            if self
                .state
                .operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
            {
                break;
            }
            changed.await;
        }
    }

    pub(crate) async fn cancel_all(&self) -> Vec<String> {
        let cancelled = self
            .state
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter_mut()
            .filter(|(_, operation)| !operation.terminal_reported)
            .map(|(operation_id, operation)| {
                operation.terminal_reported = true;
                operation.cancellation.cancel();
                operation_id.clone()
            })
            .collect();
        self.state.changed.notify_one();
        cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::InFlightOperations;
    use std::time::Duration;

    #[tokio::test]
    async fn waiter_cancellation_does_not_consume_the_terminal_notification() {
        let operations = InFlightOperations::new();
        let registered = operations.register("test");
        registered.cancellation.cancel();
        assert!(operations.finish(&registered.id).await);
        let registered = operations.register("test");
        registered.cancellation.cancel();
        assert_eq!(operations.cancel_all().await, vec![registered.id.clone()]);
        assert!(operations.cancel_all().await.is_empty());
        assert!(!operations.finish(&registered.id).await);
    }

    #[tokio::test]
    async fn cancellation_does_not_report_resources_released_until_the_operation_exits() {
        let operations = InFlightOperations::new();
        let registered = operations.register("test");
        assert_eq!(operations.cancel_all().await, vec![registered.id.clone()]);
        assert!(registered.cancellation.is_cancelled());
        assert!(!operations.wait_until_empty(Duration::ZERO).await);
        assert!(operations.cancel_all().await.is_empty());
        drop(registered);
        assert!(operations.wait_until_empty(Duration::ZERO).await);
    }
}

impl Drop for RegisteredOperation {
    fn drop(&mut self) {
        let removed = self
            .state
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.id)
            .is_some();
        if removed {
            self.state.changed.notify_one();
        }
    }
}
