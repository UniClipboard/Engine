//! 验证任务登记负责人的跨等待结果，不代表完整运行期释放。
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use uc_core::TaskRegistry;

fn available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

struct BlockingDrop {
    _lease: File,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl Drop for BlockingDrop {
    fn drop(&mut self) {
        let _ = self.entered.send(());
        let _ = self.release.recv_timeout(Duration::from_secs(10));
    }
}

fn deadline_waiter_loss(root: &Path) -> serde_json::Value {
    let path = root.join("deadline-drop.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (started, waiting_start) = mpsc::channel();
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let registry = Arc::new(TaskRegistry::new());
    runtime.block_on(async {
        assert!(
            registry
                .spawn(move |_| async move {
                    let _guard = BlockingDrop {
                        _lease: lease,
                        entered,
                        release: released,
                    };
                    started.send(()).expect("started");
                    std::future::pending::<()>().await;
                })
                .await
        );
    });
    waiting_start
        .recv_timeout(Duration::from_secs(5))
        .expect("task started");
    let first_registry = Arc::clone(&registry);
    let first =
        runtime.spawn(async move { first_registry.shutdown(Duration::from_millis(20)).await });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("drop started");
    let held = !available(&path);
    first.abort();
    let cancelled = runtime
        .block_on(first)
        .expect_err("waiter cancelled")
        .is_cancelled();
    let pending = runtime.block_on(async {
        tokio::time::timeout(Duration::from_millis(50), registry.shutdown_at(None))
            .await
            .is_err()
    });
    release.send(()).expect("release drop");
    let report = runtime.block_on(registry.shutdown_at(None));
    let repeated = runtime.block_on(registry.shutdown_at(None));
    let deadline = report.timed_out_count == 1 && report.join_error_count == 0;
    let stable = repeated.timed_out_count == report.timed_out_count
        && repeated.join_error_count == report.join_error_count;
    let free = available(&path);
    let artifact = serde_json::json!({"scope":"real deadline-triggered task destruction and independent file lock", "lock_held":held, "first_waiter_cancelled":cancelled, "second_waiter_pending":pending, "deadline_classification":deadline, "repeated_counts_equal":stable, "lock_available":free, "complete_runtime_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("task-registry-deadline-waiter.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(held && cancelled && pending && deadline && stable && free);
    artifact
}

fn executor_loss(root: &Path) -> serde_json::Value {
    let path = root.join("executor-loss.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let registry = Arc::new(TaskRegistry::new());
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("executor");
    executor.block_on(async {
        assert!(
            registry
                .spawn(move |_| async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _lease = lease;
                        entered.send(()).expect("entered");
                        released.recv().expect("release");
                    })
                    .await;
                })
                .await
        );
    });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("worker started");
    executor.shutdown_timeout(Duration::from_millis(50));
    let held = !available(&path);
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("observer");
    let report = observer.block_on(registry.shutdown_at(None));
    let repeated = observer.block_on(registry.shutdown_at(None));
    let cancelled = report
        .failures()
        .iter()
        .any(tokio::task::JoinError::is_cancelled);
    let failure = uc_application::deps::LifecycleError {
        primary: report.clone().into(),
        additional: Vec::new(),
    };
    let not_deadline = !failure.is_deadline_elapsed();
    let classified_failure = report.timed_out_count == 0 && report.join_error_count == 1;
    let repeated_same = report.timed_out_count == repeated.timed_out_count
        && report.join_error_count == repeated.join_error_count;
    let mixed = Arc::new(TaskRegistry::new());
    let old = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("mixed executor");
    let (mixed_started, mixed_waiting) = mpsc::channel();
    old.block_on(async {
        assert!(
            mixed
                .spawn(move |_| async move {
                    mixed_started.send(()).expect("mixed started");
                    std::future::pending::<()>().await;
                })
                .await
        );
    });
    mixed_waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("mixed started");
    old.shutdown_timeout(Duration::from_millis(50));
    observer.block_on(async {
        assert!(mixed.spawn(|_| std::future::pending()).await);
    });
    let mixed_report = observer.block_on(mixed.shutdown(Duration::from_millis(20)));
    let mixed_error = uc_application::deps::LifecycleError {
        primary: mixed_report.clone().into(),
        additional: Vec::new(),
    };
    let mixed_classification = mixed_report.timed_out_count == 1
        && mixed_report.join_error_count == 1
        && !mixed_error.is_deadline_elapsed();
    let deadline_registry = TaskRegistry::new();
    observer.block_on(async {
        assert!(deadline_registry.spawn(|_| std::future::pending()).await);
    });
    let deadline_report = observer.block_on(deadline_registry.shutdown(Duration::from_millis(20)));
    let deadline_failure = uc_application::deps::LifecycleError {
        primary: deadline_report.clone().into(),
        additional: Vec::new(),
    };
    let real_deadline = deadline_report.timed_out_count == 1
        && deadline_report.join_error_count == 0
        && deadline_failure.is_deadline_elapsed();
    release.send(()).expect("release");
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while !available(&path) && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    let free = available(&path);
    let artifact = serde_json::json!({"scope":"real TaskRegistry, executor loss and independent blocking-worker lock", "lock_held_after_executor_loss":held, "cancelled_join":cancelled, "timed_out_count":report.timed_out_count, "join_error_count":report.join_error_count, "cancelled_classified_as_failure":classified_failure, "not_deadline":not_deadline, "repeated_counts_equal":repeated_same, "actual_deadline_preserved":real_deadline, "mixed_failure_not_pure_deadline":mixed_classification, "lock_available_after_release":free, "complete_runtime_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("task-registry-executor-loss.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        held && cancelled
            && classified_failure
            && not_deadline
            && repeated_same
            && real_deadline
            && mixed_classification
            && free
    );
    artifact
}

fn main() {
    let root = tempfile::tempdir().expect("root");
    let executor_loss_result = executor_loss(root.path());
    let deadline_waiter_result = deadline_waiter_loss(root.path());
    let path = root.path().join("task.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let registry = Arc::new(TaskRegistry::new());
    runtime.block_on(async {
        assert!(
            registry
                .spawn(move |_| async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _lease = lease;
                        entered.send(()).expect("entered");
                        released.recv().expect("release");
                    })
                    .await;
                })
                .await
        );
    });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("worker started");
    let first_owner = Arc::clone(&registry);
    let first = runtime.spawn(async move { first_owner.shutdown_at(None).await });
    std::thread::sleep(Duration::from_millis(50));
    let held_before_cancel = !available(&path);
    first.abort();
    let first_cancelled = runtime
        .block_on(first)
        .expect_err("observer cancelled")
        .is_cancelled();
    let blocked = runtime.block_on(async {
        tokio::time::timeout(Duration::from_millis(50), registry.shutdown_at(None))
            .await
            .is_err()
    });
    let admission_closed = !runtime.block_on(registry.spawn(|_| async {}));
    let held_after_cancel = !available(&path);
    release.send(()).expect("release");
    let closed = runtime
        .block_on(registry.shutdown_at(None))
        .into_result()
        .is_ok();
    let free = available(&path);
    let failed_registry = TaskRegistry::new();
    let (panic_entered, panic_wait) = mpsc::channel();
    runtime.block_on(async {
        assert!(
            failed_registry
                .spawn(move |_| async move {
                    panic_entered.send(()).expect("panic entered");
                    panic!("controlled task panic");
                })
                .await
        );
    });
    panic_wait
        .recv_timeout(Duration::from_secs(5))
        .expect("panic task started");
    let first_report = runtime.block_on(failed_registry.shutdown_at(None));
    let first_failed = first_report
        .failures()
        .iter()
        .any(tokio::task::JoinError::is_panic);
    let second_report = runtime.block_on(failed_registry.shutdown_at(None));
    let second_failed = second_report
        .failures()
        .iter()
        .any(tokio::task::JoinError::is_panic);
    let same_counts = first_report.completed_count == second_report.completed_count
        && first_report.timed_out_count == second_report.timed_out_count
        && first_report.join_error_count == second_report.join_error_count;
    let same_source = first_report
        .failures()
        .first()
        .zip(second_report.failures().first())
        .is_some_and(|(first, second)| std::ptr::eq(first, second));
    let mixed_path = root.path().join("mixed.lease");
    let mixed_lease = File::create(&mixed_path).expect("mixed lease");
    mixed_lease.lock().expect("mixed lock");
    let (mixed_entered, mixed_wait) = mpsc::channel();
    let (mixed_release, mixed_released) = mpsc::channel();
    let mixed = Arc::new(TaskRegistry::new());
    runtime.block_on(async {
        assert!(
            mixed
                .spawn(|_| async {
                    panic!("controlled mixed panic");
                })
                .await
        );
        assert!(
            mixed
                .spawn(move |_| async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _lease = mixed_lease;
                        mixed_entered.send(()).expect("mixed entered");
                        mixed_released.recv().expect("mixed release");
                    })
                    .await;
                })
                .await
        );
    });
    mixed_wait
        .recv_timeout(Duration::from_secs(5))
        .expect("mixed started");
    let first_mixed = Arc::clone(&mixed);
    let mixed_waiter = runtime.spawn(async move { first_mixed.shutdown_at(None).await });
    std::thread::sleep(Duration::from_millis(100));
    mixed_waiter.abort();
    let mixed_cancelled = runtime
        .block_on(mixed_waiter)
        .expect_err("mixed observer cancelled")
        .is_cancelled();
    let mixed_held = !available(&mixed_path);
    mixed_release.send(()).expect("mixed release");
    let mixed_report = runtime.block_on(mixed.shutdown_at(None));
    let mixed_repeat = runtime.block_on(mixed.shutdown_at(None));
    let mixed_failure = mixed_report
        .failures()
        .iter()
        .any(tokio::task::JoinError::is_panic)
        && mixed_repeat
            .failures()
            .iter()
            .any(tokio::task::JoinError::is_panic)
        && available(&mixed_path);
    let result = serde_json::json!({"executor_loss":executor_loss_result, "deadline_waiter":deadline_waiter_result, "scope":"real TaskRegistry, controlled blocking worker and independent lock", "lock_held_before_cancel":held_before_cancel, "first_observer_cancelled":first_cancelled, "later_observer_pending":blocked, "admission_closed":admission_closed, "lock_held_after_cancel":held_after_cancel, "completed_after_release":closed, "lock_available":free, "first_panic_retained":first_failed, "second_panic_retained":second_failed, "repeated_counts_equal":same_counts, "same_failure_source":same_source, "mixed_observer_cancelled":mixed_cancelled, "mixed_lock_held":mixed_held, "mixed_failure_retained":mixed_failure, "complete_runtime_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("task-registry-release.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        held_before_cancel
            && first_cancelled
            && blocked
            && admission_closed
            && held_after_cancel
            && closed
            && free
            && first_failed
            && second_failed
            && same_counts
            && same_source
            && mixed_cancelled
            && mixed_held
            && mixed_failure
    );
}
