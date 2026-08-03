use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;

use super::runtime::{HistoryMaintenance, HistoryMaintenanceRuntime};
use super::{
    CleanupResultView, ClipboardHistoryError, ReconcileResultView, RetentionEnforcementResultView,
};

struct FakeHistoryMaintenance {
    calls: Mutex<Vec<&'static str>>,
    calls_changed: Notify,
    reconcile_failures_remaining: AtomicUsize,
    cleanup_fails: AtomicBool,
    retention_fails: AtomicBool,
    block_cleanup: AtomicBool,
    cleanup_started: Notify,
    cleanup_release: Notify,
}

impl FakeHistoryMaintenance {
    fn new(reconcile_failures: usize, cleanup_fails: bool, retention_fails: bool) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            calls_changed: Notify::new(),
            reconcile_failures_remaining: AtomicUsize::new(reconcile_failures),
            cleanup_fails: AtomicBool::new(cleanup_fails),
            retention_fails: AtomicBool::new(retention_fails),
            block_cleanup: AtomicBool::new(false),
            cleanup_started: Notify::new(),
            cleanup_release: Notify::new(),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn record(&self, call: &'static str) {
        self.calls.lock().expect("calls lock").push(call);
        self.calls_changed.notify_waiters();
    }

    async fn wait_for_call_count(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let changed = self.calls_changed.notified();
                if self.calls().len() >= expected {
                    return;
                }
                changed.await;
            }
        })
        .await
        .expect("maintenance calls reached expected count");
    }

    fn consume_reconcile_failure(&self) -> bool {
        self.reconcile_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
}

#[async_trait]
impl HistoryMaintenance for FakeHistoryMaintenance {
    async fn reconcile_missing_files(&self) -> Result<ReconcileResultView, ClipboardHistoryError> {
        self.record("reconcile");
        if self.consume_reconcile_failure() {
            Err(ClipboardHistoryError::Internal("probe".into()))
        } else {
            Ok(ReconcileResultView::default())
        }
    }

    async fn cleanup_expired_files(&self) -> Result<CleanupResultView, ClipboardHistoryError> {
        self.record("cleanup");
        if self.block_cleanup.load(Ordering::SeqCst) {
            self.cleanup_started.notify_one();
            self.cleanup_release.notified().await;
        }
        if self.cleanup_fails.load(Ordering::SeqCst) {
            Err(ClipboardHistoryError::Internal("probe".into()))
        } else {
            Ok(CleanupResultView::default())
        }
    }

    async fn enforce_retention_policy(
        &self,
    ) -> Result<RetentionEnforcementResultView, ClipboardHistoryError> {
        self.record("retention");
        if self.retention_fails.load(Ordering::SeqCst) {
            Err(ClipboardHistoryError::Internal("probe".into()))
        } else {
            Ok(RetentionEnforcementResultView::default())
        }
    }
}

fn maintenance_port(maintenance: &Arc<FakeHistoryMaintenance>) -> Arc<dyn HistoryMaintenance> {
    maintenance.clone()
}

#[tokio::test]
async fn runtime_keeps_fixed_order_when_later_passes_fail() {
    let maintenance = Arc::new(FakeHistoryMaintenance::new(0, true, true));

    let runtime = HistoryMaintenanceRuntime::start_with_interval(
        maintenance_port(&maintenance),
        Duration::from_secs(3_600),
    )
    .await;

    assert_eq!(
        maintenance.calls(),
        vec!["reconcile", "cleanup", "retention"]
    );
    runtime.shutdown().await.expect("runtime shutdown");
}

#[tokio::test]
async fn runtime_skips_delete_passes_when_reconciliation_fails() {
    let maintenance = Arc::new(FakeHistoryMaintenance::new(1, false, false));

    let runtime = HistoryMaintenanceRuntime::start_with_interval(
        maintenance_port(&maintenance),
        Duration::from_secs(3_600),
    )
    .await;

    assert_eq!(maintenance.calls(), vec!["reconcile"]);
    runtime.shutdown().await.expect("runtime shutdown");
}

#[tokio::test]
async fn periodic_pass_retries_after_a_failed_startup_pass() {
    let maintenance = Arc::new(FakeHistoryMaintenance::new(1, false, false));
    let runtime = HistoryMaintenanceRuntime::start_with_interval(
        maintenance_port(&maintenance),
        Duration::from_millis(5),
    )
    .await;

    maintenance.wait_for_call_count(4).await;

    runtime.shutdown().await.expect("runtime shutdown");
    assert_eq!(
        &maintenance.calls()[..4],
        &["reconcile", "reconcile", "cleanup", "retention"]
    );
}

#[tokio::test]
async fn shutdown_interrupts_the_long_interval_wait() {
    let maintenance = Arc::new(FakeHistoryMaintenance::new(0, false, false));
    let runtime = HistoryMaintenanceRuntime::start_with_interval(
        maintenance_port(&maintenance),
        Duration::from_secs(3_600),
    )
    .await;

    tokio::time::timeout(Duration::from_millis(100), runtime.shutdown())
        .await
        .expect("shutdown did not wait for the interval")
        .expect("runtime shutdown");
}

#[tokio::test]
async fn shutdown_waits_for_an_inflight_pass_to_finish() {
    let maintenance = Arc::new(FakeHistoryMaintenance::new(0, false, false));
    let runtime = HistoryMaintenanceRuntime::start_with_interval(
        maintenance_port(&maintenance),
        Duration::from_millis(5),
    )
    .await;
    maintenance.block_cleanup.store(true, Ordering::SeqCst);
    tokio::time::timeout(
        Duration::from_secs(1),
        maintenance.cleanup_started.notified(),
    )
    .await
    .expect("periodic cleanup started");

    let mut shutdown = tokio::spawn(async move { runtime.shutdown().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut shutdown)
            .await
            .is_err()
    );

    maintenance.cleanup_release.notify_one();
    shutdown
        .await
        .expect("shutdown task")
        .expect("runtime shutdown");
}
