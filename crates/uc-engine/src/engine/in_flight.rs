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
    operations: Mutex<HashMap<String, CancellationToken>>,
    changed: Notify,
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

    pub(crate) async fn register(&self, prefix: &str) -> RegisteredOperation {
        let id = format!("{prefix}-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let cancellation = CancellationToken::new();
        self.state
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), cancellation.clone());
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
        removed.is_some_and(|cancellation| !cancellation.is_cancelled())
    }

    pub(crate) async fn wait_until_empty(&self, deadline: Duration) -> bool {
        tokio::time::timeout(deadline, async {
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
        })
        .await
        .is_ok()
    }

    pub(crate) async fn cancel_all(&self) -> Vec<String> {
        let cancelled = self
            .state
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|(_, cancellation)| !cancellation.is_cancelled())
            .map(|(operation_id, cancellation)| {
                cancellation.cancel();
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
    async fn cancellation_does_not_report_resources_released_until_the_operation_exits() {
        let operations = InFlightOperations::new();
        let registered = operations.register("test").await;
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
