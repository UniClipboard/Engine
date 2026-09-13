//! # Task Registry
//!
//! Centralized async task lifecycle management using `CancellationToken` + `JoinSet`.
//!
//! All long-lived spawned tasks are tracked here, enabling graceful shutdown
//! with cooperative cancellation and bounded join timeout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{error::Error, fmt};
use tokio::task::{JoinError, JoinSet};
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct TaskShutdownReport {
    pub completed_count: usize,
    pub timed_out_count: usize,
    pub join_error_count: usize,
    failures: Vec<JoinError>,
}

impl TaskShutdownReport {
    pub fn into_result(self) -> Result<(), Self> {
        if self.failures.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }

    pub fn failures(&self) -> &[JoinError] {
        &self.failures
    }
}

impl fmt::Debug for TaskShutdownReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TaskShutdownReport")
            .field("completed_count", &self.completed_count)
            .field("timed_out_count", &self.timed_out_count)
            .field("join_error_count", &self.join_error_count)
            .finish()
    }
}

impl fmt::Display for TaskShutdownReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("background tasks did not stop cleanly")
    }
}

impl Error for TaskShutdownReport {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.failures
            .first()
            .map(|source| source as &(dyn Error + 'static))
    }
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
    /// 3. 宽限时间耗尽后取消剩余任务，并等待析构完成；不能把取消请求当作资源已释放。
    pub async fn shutdown(&self, timeout_duration: Duration) -> TaskShutdownReport {
        self.shutdown_at(Instant::now().checked_add(timeout_duration))
            .await
    }

    /// 共用调用方的绝对期限，排队不能重新获得宽限时间；None 表示不设期限。
    pub async fn shutdown_at(&self, deadline: Option<Instant>) -> TaskShutdownReport {
        self.closed.store(true, Ordering::Release);
        self.token.cancel();

        let mut tasks = self.tasks.lock().await;
        let deadline = async {
            match deadline {
                Some(deadline) => sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(deadline);
        let mut report = TaskShutdownReport::default();

        loop {
            tokio::select! {
                biased;
                _ = &mut deadline => {
                    // 截止已到时先固定进入收尾；已经完成的任务仍在这里完整计入报告。
                    while let Some(result) = tasks.try_join_next() {
                        add_join_result(&mut report, result);
                    }
                    tasks.abort_all();
                    while let Some(result) = tasks.join_next().await {
                        add_join_result(&mut report, result);
                    }
                    return report;
                }
                result = tasks.join_next() => {
                    match result {
                        Some(result) => add_join_result(&mut report, result),
                        None => return report,
                    }
                }
            }
        }
    }
}

fn add_join_result(report: &mut TaskShutdownReport, result: Result<(), JoinError>) {
    match result {
        Ok(()) => report.completed_count += 1,
        Err(error) => {
            if error.is_cancelled() {
                report.timed_out_count += 1;
            } else {
                report.join_error_count += 1;
            }
            report.failures.push(error);
        }
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
        let failure = report.into_result().unwrap_err();
        assert!(failure
            .source()
            .unwrap()
            .downcast_ref::<JoinError>()
            .unwrap()
            .is_panic());
        assert_eq!(failure.failures().len(), 1);
        assert!(!failure.to_string().contains("PRIVATE_TASK_PANIC"));
    }

    #[tokio::test]
    async fn shutdown_aborts_tasks_that_ignore_cancellation_after_the_deadline() {
        let registry = TaskRegistry::new();
        let resource = Arc::new(());
        let held = Arc::clone(&resource);
        assert!(
            registry
                .spawn(move |_| async move {
                    let _held = held;
                    std::future::pending::<()>().await;
                })
                .await
        );

        let report = registry.shutdown(Duration::from_millis(1)).await;

        assert_eq!(report.timed_out_count, 1);
        assert_eq!(report.completed_count, 0);
        assert_eq!(report.join_error_count, 0);
        assert_eq!(Arc::strong_count(&resource), 1, "关闭完成必须释放任务资源");
    }

    #[tokio::test]
    async fn shutdown_retains_every_failure_with_redacted_summary() {
        let registry = TaskRegistry::new();
        assert!(
            registry
                .spawn(|_| async { panic!("PRIVATE_FAILURE") })
                .await
        );
        assert!(registry.spawn(|_| std::future::pending()).await);
        let report = registry
            .shutdown(Duration::from_millis(10))
            .await
            .into_result()
            .unwrap_err();
        assert_eq!(report.failures().len(), 2);
        assert_eq!(report.join_error_count, 1);
        assert_eq!(report.timed_out_count, 1);
        assert!(report.failures().iter().any(JoinError::is_panic));
        assert!(report.failures().iter().any(JoinError::is_cancelled));
        assert!(!format!("{report:?}").contains("PRIVATE_FAILURE"));
    }

    #[tokio::test]
    async fn spawn_is_rejected_after_shutdown_closes_the_registry() {
        let registry = TaskRegistry::new();
        let report = registry.shutdown(Duration::ZERO).await;
        assert!(report.into_result().is_ok());

        assert!(!registry.spawn(|_| async {}).await);
        assert_eq!(registry.task_count().await, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_includes_time_waiting_for_the_task_lock() {
        let registry = Arc::new(TaskRegistry::new());
        let resource = Arc::new(());
        let held = Arc::clone(&resource);
        assert!(
            registry
                .spawn(move |_| async move {
                    let _held = held;
                    std::future::pending::<()>().await;
                })
                .await
        );
        let guard = registry.tasks.lock().await;
        let started = Instant::now();
        let closing = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.shutdown(Duration::from_secs(1)).await }
        });
        registry.token.cancelled().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        drop(guard);

        let report = closing.await.unwrap();
        assert_eq!(report.timed_out_count, 1);
        assert!(started.elapsed() < Duration::from_millis(2100));
        assert_eq!(Arc::strong_count(&resource), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn sequential_registries_share_the_original_deadline() {
        let first = TaskRegistry::new();
        let second = TaskRegistry::new();
        assert!(first.spawn(|_| std::future::pending()).await);
        assert!(second.spawn(|_| std::future::pending()).await);
        let started = Instant::now();
        let deadline = Some(started + Duration::from_secs(1));
        assert_eq!(first.shutdown_at(deadline).await.timed_out_count, 1);
        assert_eq!(second.shutdown_at(deadline).await.timed_out_count, 1);
        assert!(started.elapsed() < Duration::from_millis(1100));
    }
}
