//! 验证网络恢复工作中断后，本地资源保持由关闭负责人持有。
#[path = "../src/runtime/shutdown/coordination.rs"]
mod coordination;

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use coordination::{stop_runtime_resource_users, ShutdownActions};
use tokio::time::Instant;
use uc_application::facade::{
    NetworkRecoveryFacade, NetworkRecoveryRequestError, RebuildNetworkSessionError,
    RebuildNetworkSessionPort,
};

struct Rebuilder(Mutex<Option<(File, mpsc::Sender<()>, mpsc::Receiver<()>)>>);
#[async_trait]
impl RebuildNetworkSessionPort for Rebuilder {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        let (worker, entered, released) = self.0.lock().expect("rebuilder").take().expect("worker");
        let task = tokio::task::spawn_blocking(move || {
            let _worker = worker;
            entered.send(()).expect("worker entered");
            released.recv().expect("worker released");
        });
        task.await
            .map_err(|error| RebuildNetworkSessionError::new(error, false))?;
        Ok(())
    }
}
struct FailedRebuilder;
#[async_trait]
impl RebuildNetworkSessionPort for FailedRebuilder {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        Err(RebuildNetworkSessionError::new(
            std::io::Error::other("controlled failure"),
            false,
        ))
    }
}
struct Actions(Mutex<Option<File>>);
#[async_trait]
impl ShutdownActions for Actions {
    async fn stop_session(&self, _: Option<Instant>) -> anyhow::Result<()> {
        Ok(())
    }
    async fn close_file_transfers(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn stop_process_tasks(&self, _: Option<Instant>) -> anyhow::Result<()> {
        Ok(())
    }
    fn close_local_resources(&self) {
        self.0.lock().expect("resource").take();
    }
}
fn available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}
fn locked(path: &Path) -> File {
    let file = File::create(path).expect("file");
    file.lock().expect("lock");
    file
}
fn main() {
    let root = tempfile::tempdir().expect("temporary directory");
    let worker_path = root.path().join("worker.lock");
    let resource_path = root.path().join("resource.lock");
    let (entered, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let port = Arc::new(Rebuilder(Mutex::new(Some((
        locked(&worker_path),
        entered,
        released,
    )))));
    let recovery = NetworkRecoveryFacade::new(port);
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("executor");
    executor.spawn({
        let recovery = recovery.clone();
        async move {
            let _ = recovery.request_recovery().await;
        }
    });
    ready
        .recv_timeout(Duration::from_secs(5))
        .expect("worker ready");
    executor.shutdown_timeout(Duration::from_millis(50));
    let actions = Actions(Mutex::new(Some(locked(&resource_path))));
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("observer");
    let first = observer.block_on(stop_runtime_resource_users(
        async { recovery.shutdown().await.map_err(anyhow::Error::new) },
        &actions,
        None,
    ));
    let worker_held = !available(&worker_path);
    let resource_held = !available(&resource_path);
    let task_error = first.errors.first().and_then(|error| error.downcast_ref::<NetworkRecoveryRequestError>()).is_some_and(|error| matches!(error, NetworkRecoveryRequestError::Task(source) if source.is_cancelled()));
    release.send(()).expect("release worker");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !available(&worker_path) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let worker_free = available(&worker_path);
    let repeated = observer.block_on(stop_runtime_resource_users(
        async { recovery.shutdown().await.map_err(anyhow::Error::new) },
        &actions,
        None,
    ));
    let repeated_failed = repeated.errors.first().and_then(|error| error.downcast_ref::<NetworkRecoveryRequestError>()).is_some_and(|error| matches!(error, NetworkRecoveryRequestError::Task(source) if source.is_cancelled()));
    let still_held = !available(&resource_path);
    let operation_path = root.path().join("operation.lock");
    let completed_actions = Actions(Mutex::new(Some(locked(&operation_path))));
    let completed_recovery = NetworkRecoveryFacade::new(Arc::new(FailedRebuilder));
    let operation_failed = observer
        .block_on(completed_recovery.request_recovery())
        .is_err();
    let completed = observer.block_on(stop_runtime_resource_users(
        async {
            completed_recovery
                .shutdown()
                .await
                .map_err(anyhow::Error::new)
        },
        &completed_actions,
        None,
    ));
    let operation_closed = completed.errors.is_empty() && available(&operation_path);
    let classified_path = root.path().join("classified.lock");
    let classified_actions = Actions(Mutex::new(Some(locked(&classified_path))));
    let classified = observer.block_on(stop_runtime_resource_users(
        async {
            Err(anyhow::Error::new(NetworkRecoveryRequestError::Rebuild(
                RebuildNetworkSessionError::new(std::io::Error::other("controlled failure"), false),
            )))
        },
        &classified_actions,
        None,
    ));
    let completed_error_retained = classified.errors.len() == 1 && !available(&classified_path);
    let unknown_path = root.path().join("unknown.lock");
    let unknown_actions = Actions(Mutex::new(Some(locked(&unknown_path))));
    let unknown = observer.block_on(stop_runtime_resource_users(
        async { Err(anyhow::anyhow!("unclassified failure")) },
        &unknown_actions,
        None,
    ));
    let unknown_retained = unknown.errors.len() == 1 && !available(&unknown_path);
    let artifact = serde_json::json!({"scope":"production network recovery and shutdown coordinator, controlled rebuilder and independent file locks", "worker_held_after_executor_loss":worker_held, "local_resource_retained":resource_held, "cancelled_worker_source_retained":task_error, "worker_eventually_closed":worker_free, "repeated_result_unconfirmed":repeated_failed, "resource_remains_retained":still_held, "completed_operation_error_recorded":operation_failed, "successful_recovery_shutdown_allows_close":operation_closed, "rebuild_error_retains_resource":completed_error_retained, "unknown_error_retains_resource":unknown_retained, "complete_engine_release":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("artifact directory");
        std::fs::write(
            Path::new(&dir).join("network-recovery-shutdown.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        worker_held
            && resource_held
            && task_error
            && worker_free
            && repeated_failed
            && still_held
            && operation_failed
            && operation_closed
            && completed_error_retained
            && unknown_retained
    );
}
