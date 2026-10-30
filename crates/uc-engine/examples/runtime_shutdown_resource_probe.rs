//! 验证关闭协调器对未确认任务的资源保留；不代表完整运行期释放。
#[path = "../src/runtime/shutdown/coordination.rs"]
mod coordination;

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use coordination::{stop_resource_users_and_close, ShutdownActions};
use tokio::time::Instant;
use uc_core::{TaskRegistry, TaskShutdownReport};

struct Actions {
    tasks: Arc<TaskRegistry>,
    resource: Mutex<Option<File>>,
    session_stops: AtomicUsize,
    transfer_stops: AtomicUsize,
}

#[async_trait]
impl ShutdownActions for Actions {
    async fn stop_session(&self, _: Option<Instant>) -> anyhow::Result<()> {
        self.session_stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn close_file_transfers(&self) -> anyhow::Result<()> {
        self.transfer_stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn stop_process_tasks(&self, deadline: Option<Instant>) -> anyhow::Result<()> {
        self.tasks
            .shutdown_at(deadline)
            .await
            .into_result()
            .map_err(Into::into)
    }
    fn close_local_resources(&self) {
        self.resource.lock().expect("resource").take();
    }
}

fn available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

fn main() {
    let root = tempfile::tempdir().expect("temporary directory");
    let worker_path = root.path().join("worker.lock");
    let resource_path = root.path().join("resource.lock");
    let worker = File::create(&worker_path).expect("worker file");
    worker.lock().expect("worker lock");
    let resource = File::create(&resource_path).expect("resource file");
    resource.lock().expect("resource lock");
    let tasks = Arc::new(TaskRegistry::new());
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("executor");
    let (entered, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    executor.block_on(async {
        assert!(
            tasks
                .spawn(move |_| async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _worker = worker;
                        entered.send(()).expect("worker entered");
                        released.recv().expect("worker released");
                    })
                    .await;
                })
                .await
        );
    });
    ready
        .recv_timeout(Duration::from_secs(5))
        .expect("worker ready");
    executor.shutdown_timeout(Duration::from_millis(50));
    let actions = Actions {
        tasks,
        resource: Mutex::new(Some(resource)),
        session_stops: AtomicUsize::new(0),
        transfer_stops: AtomicUsize::new(0),
    };
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("observer");
    let first = observer.block_on(stop_resource_users_and_close(&actions, None));
    let worker_held = !available(&worker_path);
    let resource_held = !available(&resource_path);
    let first_failed = first.errors.len() == 1 && !first.process_tasks_stopped;
    let error_retained = first
        .errors
        .first()
        .and_then(|error| error.downcast_ref::<TaskShutdownReport>())
        .is_some_and(|report| {
            report
                .failures()
                .iter()
                .any(tokio::task::JoinError::is_cancelled)
        });
    release.send(()).expect("release worker");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !available(&worker_path) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let worker_free = available(&worker_path);
    let repeated = observer.block_on(stop_resource_users_and_close(&actions, None));
    let repeated_failed = repeated.errors.len() == 1 && !repeated.process_tasks_stopped;
    let still_held = !available(&resource_path);
    let other_owners_stopped = actions.session_stops.load(Ordering::SeqCst) == 2
        && actions.transfer_stops.load(Ordering::SeqCst) == 2;
    let success_path = root.path().join("success.lock");
    let success_file = File::create(&success_path).expect("success file");
    success_file.lock().expect("success lock");
    let success = Actions {
        tasks: Arc::new(TaskRegistry::new()),
        resource: Mutex::new(Some(success_file)),
        session_stops: AtomicUsize::new(0),
        transfer_stops: AtomicUsize::new(0),
    };
    let outcome = observer.block_on(stop_resource_users_and_close(&success, None));
    let success_closed = outcome.errors.is_empty() && available(&success_path);
    let artifact = serde_json::json!({"scope":"production shutdown coordination, real task registry and independent file locks", "worker_held_after_executor_loss":worker_held, "local_resource_retained":resource_held, "first_result_unconfirmed":first_failed, "cancelled_task_source_retained":error_retained, "other_owners_stopped":other_owners_stopped, "worker_eventually_closed":worker_free, "repeated_result_unconfirmed":repeated_failed, "resource_remains_retained":still_held, "successful_shutdown_closes_resource":success_closed, "complete_engine_release":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("artifact directory");
        std::fs::write(
            Path::new(&dir).join("runtime-shutdown-resource.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        worker_held
            && resource_held
            && first_failed
            && error_retained
            && other_owners_stopped
            && worker_free
            && repeated_failed
            && still_held
            && success_closed
    );
}
