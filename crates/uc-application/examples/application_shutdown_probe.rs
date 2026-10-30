//! 验证 Application 关闭负责人跨执行器保有已接收工作，不代表完整 Engine 释放。
#![cfg(not(test))]
#![allow(dead_code)]
use std::fs::File;
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::time::Instant;

mod runtime_lifecycle {
    pub use uc_application::deps::LifecycleError;
}

// 只满足实际协调模块的装配类型；本场景不运行这些领域关闭器。
struct Stub;
impl Stub {
    async fn shutdown(self) -> Result<(), io::Error> {
        Ok(())
    }
    async fn on_shutdown(&self) -> Result<(), io::Error> {
        Ok(())
    }
}
struct TimeoutStub;
impl TimeoutStub {
    async fn shutdown(self, _: Option<Instant>) -> Result<(), io::Error> {
        Ok(())
    }
}
struct ApplicationRuntimeOwners {
    progress: Stub,
    history_maintenance: Stub,
    file_transfer_timeout: TimeoutStub,
    search: Stub,
    space: Stub,
    clipboard: Stub,
    active_clipboard: Stub,
}
#[path = "../src/application/shutdown.rs"]
mod shutdown;

fn available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

fn main() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("cleanup.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let owner = Arc::new(shutdown::ApplicationShutdown::default());
    let old = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("old runtime");
    let first_owner = Arc::clone(&owner);
    let caller = old.spawn(async move {
        first_owner
            .run(async move {
                tokio::task::spawn_blocking(move || {
                    let _lease = lease;
                    entered.send(()).expect("entered");
                    released.recv().expect("release");
                })
                .await
                .map_err(|source| runtime_lifecycle::LifecycleError {
                    primary: source.into(),
                    additional: Vec::new(),
                })?;
                Ok(())
            })
            .await
    });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("cleanup started");
    assert!(!caller.is_finished());
    old.shutdown_timeout(Duration::from_millis(50));
    let held_after_executor = !available(&path);
    let next = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("observer runtime");
    let waiting_result = next.block_on(async {
        tokio::time::timeout(
            Duration::from_millis(100),
            owner.run(async { panic!("must join accepted cleanup") }),
        )
        .await
    });
    let pending = waiting_result.is_err();
    let early_failure = matches!(waiting_result, Ok(Err(_)));
    let second_pending = next.block_on(async {
        tokio::time::timeout(
            Duration::from_millis(50),
            owner.run(async { panic!("must join cleanup") }),
        )
        .await
        .is_err()
    });
    let held_after_wait = !available(&path);
    release.send(()).expect("release");
    let final_result = next.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            owner.run(async { panic!("must not start another cleanup") }),
        )
        .await
    });
    let final_ok = matches!(final_result, Ok(Ok(())));
    let repeat_ok = matches!(
        next.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(1),
                owner.run(async { panic!("must not restart cleanup") }),
            )
            .await
        }),
        Ok(Ok(()))
    );
    let free = available(&path);
    let failure_owner = shutdown::ApplicationShutdown::default();
    let first_failure = next
        .block_on(failure_owner.run(async {
            Err(runtime_lifecycle::LifecycleError {
                primary: io::Error::new(io::ErrorKind::PermissionDenied, "controlled").into(),
                additional: Vec::new(),
            })
        }))
        .expect_err("cleanup fails");
    let second_failure = next
        .block_on(failure_owner.run(async { Ok(()) }))
        .expect_err("failure is stable");
    let failure_retained = Arc::ptr_eq(&first_failure, &second_failure)
        && second_failure
            .primary
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied);
    let panic_owner = shutdown::ApplicationShutdown::default();
    let panic_failure = next
        .block_on(panic_owner.run(async { panic!("controlled cleanup panic") }))
        .expect_err("panic is failure");
    let panic_repeat = next
        .block_on(panic_owner.run(async { Ok(()) }))
        .expect_err("panic remains failure");
    let panic_retained = Arc::ptr_eq(&panic_failure, &panic_repeat)
        && panic_failure
            .primary
            .downcast_ref::<tokio::task::JoinError>()
            .is_some_and(|source| source.is_panic());
    let result = serde_json::json!({"scope":"production Application shutdown coordinator, controlled blocking worker, independent lock probe", "lock_held_after_executor":held_after_executor, "pending_after_observer_timeout":pending, "second_waiter_pending":second_pending, "failure_source_retained":failure_retained, "panic_source_retained":panic_retained, "early_terminal_failure":early_failure, "lock_held_after_wait":held_after_wait, "final_success":final_ok, "repeat_success":repeat_ok, "lock_available":free, "complete_application_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("application-shutdown-executor.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        held_after_executor
            && pending
            && second_pending
            && failure_retained
            && panic_retained
            && !early_failure
            && held_after_wait
            && final_ok
            && repeat_ok
            && free
    );
}
