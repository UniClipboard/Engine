//! # Task Registry
//!
//! Centralized async task lifecycle management using `CancellationToken` + `JoinSet`.
//!
//! All long-lived spawned tasks are tracked here, enabling graceful shutdown
//! with cooperative cancellation and bounded join timeout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskShutdownReport {
    pub completed_count: usize,
    pub timed_out_count: usize,
    pub join_error_count: usize,
}

/// Centralized registry for tracking and managing long-lived async tasks.
///
/// Provides:
/// - `spawn()` to track tasks with a `CancellationToken` for cooperative shutdown
/// - `shutdown()` to cancel all tasks and join with a bounded timeout
/// - `child_token()` for creating subordinate cancellation tokens
/// - `token()` for direct access to the root cancellation token
pub struct TaskRegistry {
    token: CancellationToken,
    tasks: tokio::sync::Mutex<JoinSet<()>>,
    closed: AtomicBool,
}

impl TaskRegistry {
    /// Create a new empty TaskRegistry.
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            tasks: tokio::sync::Mutex::new(JoinSet::new()),
            closed: AtomicBool::new(false),
        }
    }

    /// Get a child token that is cancelled when the root token is cancelled.
    pub fn child_token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// Get a reference to the root cancellation token.
    ///
    /// Used by the app exit hook to signal shutdown without calling the full
    /// `shutdown()` method (which requires async context).
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Spawn a tracked task that receives a `CancellationToken` for cooperative cancellation.
    ///
    /// The task is added to the internal `JoinSet` and will be joined during `shutdown()`.
    /// A child token is created for each task so cancelling the root token cascades.
    pub async fn spawn<F, Fut>(&self, f: F) -> bool
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let mut tasks = self.tasks.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let token = self.token.child_token();
        tasks.spawn(async move { f(token).await });
        true
    }

    /// Returns the number of currently tracked tasks.
    pub async fn task_count(&self) -> usize {
        self.tasks.lock().await.len()
    }

    /// Cancel all tracked tasks and join with a bounded timeout.
    ///
    /// 1. Cancels the root token (propagates to all child tokens)
    /// 2. Awaits `join_next()` in a loop with a deadline
    /// 3. If the deadline fires before all tasks join, aborts the remaining tasks
    pub async fn shutdown(&self, timeout_duration: Duration) -> TaskShutdownReport {
        self.closed.store(true, Ordering::Release);
        self.token.cancel();

        let mut tasks = self.tasks.lock().await;
        let deadline = tokio::time::sleep(timeout_duration);
        tokio::pin!(deadline);
        let mut report = TaskShutdownReport::default();

        loop {
            tokio::select! {
                result = tasks.join_next() => {
                    match result {
                        Some(Ok(())) => report.completed_count += 1,
                        Some(Err(error)) if error.is_cancelled() => report.timed_out_count += 1,
                        Some(Err(_)) => report.join_error_count += 1,
                        None => return report,
                    }
                }
                _ = &mut deadline => {
                    while let Some(result) = tasks.try_join_next() {
                        add_join_result(&mut report, result);
                    }
                    let remaining = tasks.len();
                    tasks.abort_all();
                    tasks.detach_all();
                    report.timed_out_count = report.timed_out_count.saturating_add(remaining);
                    return report;
                }
            }
        }
    }
}

fn add_join_result(report: &mut TaskShutdownReport, result: Result<(), tokio::task::JoinError>) {
    match result {
        Ok(()) => report.completed_count += 1,
        Err(error) if error.is_cancelled() => report.timed_out_count += 1,
        Err(_) => report.join_error_count += 1,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn shutdown_reports_completed_panicked_and_aborted_tasks_without_error_text() {
        let registry = Arc::new(TaskRegistry::new());
        assert!(registry.spawn(|_| async {}).await);
        assert!(
            registry
                .spawn(|_| async { panic!("PRIVATE_TASK_PANIC") })
                .await
        );
        assert!(
            registry
                .spawn(|cancel| async move { cancel.cancelled().await })
                .await
        );

        let report = registry.shutdown(Duration::from_secs(1)).await;

        assert_eq!(report.completed_count, 2);
        assert_eq!(report.join_error_count, 1);
        assert_eq!(report.timed_out_count, 0);
        assert!(!format!("{report:?}").contains("PRIVATE_TASK_PANIC"));
    }

    #[tokio::test]
    async fn shutdown_aborts_tasks_that_ignore_cancellation_after_the_deadline() {
        let registry = TaskRegistry::new();
        assert!(
            registry
                .spawn(|_| async {
                    std::future::pending::<()>().await;
                })
                .await
        );

        let report = registry.shutdown(Duration::from_millis(1)).await;

        assert_eq!(report.timed_out_count, 1);
        assert_eq!(report.completed_count, 0);
        assert_eq!(report.join_error_count, 0);
    }

    #[tokio::test]
    async fn spawn_is_rejected_after_shutdown_closes_the_registry() {
        let registry = TaskRegistry::new();
        let report = registry.shutdown(Duration::ZERO).await;
        assert_eq!(report, TaskShutdownReport::default());

        assert!(!registry.spawn(|_| async {}).await);
        assert_eq!(registry.task_count().await, 0);
    }
}
