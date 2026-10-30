//! 失败清单：放弃启动后，清理失败或运行时结束不能丢弃 closer，也不能允许重新启动。
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::time::timeout;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboard, HostClipboardChange, HostClipboardChangeStream,
    HostClipboardSnapshot, HostDirectories, Operation, SecretString, StartupLifecycle,
    StartupProgress, StartupState,
};

use super::{host, EmptyClipboard, EmptyFiles, HeldSecureStorage, MemorySecureStorage};

struct FailedCloser(File);

#[async_trait]
impl HostClipboardChangeStream for FailedCloser {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        let _ = self.0.metadata();
        Err(HostCapabilityError::new(
            HostCapabilityErrorCategory::Io,
            "controlled failure",
        ))
    }
}

struct Clipboard(Mutex<Option<Box<dyn HostClipboardChangeStream>>>);

impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        EmptyClipboard.read()
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        EmptyClipboard.write(snapshot)
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self.0.lock().unwrap().take())
    }
}

fn available(path: &Path) -> bool {
    let status = Command::new("python3")
        .args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(path)
        .status()
        .expect("independent lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

fn run_isolated(test: &str) -> bool {
    const CHILD: &str = "UC_TEST_ABANDONED_CLEANUP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(CHILD, "1")
        .status()
        .unwrap();
    assert!(status.success());
    true
}

struct BlockedCloser {
    _lease: File,
    entered: mpsc::Sender<()>,
}

#[async_trait]
impl HostClipboardChangeStream for BlockedCloser {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        let _ = self.entered.send(());
        std::future::pending().await
    }
}

#[test]
fn abandoned_startup_keeps_closer_when_executor_stops() {
    if run_isolated(
        "startup::abandoned_cleanup::abandoned_startup_keeps_closer_when_executor_stops",
    ) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (engine, _) = Engine::start(
            EngineConfig::new("2.0.0"),
            host(root.path(), Box::new(storage.clone())),
        )
        .await
        .unwrap();
        engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("startup executor test".into()),
                passphrase: SecretString::new("startup-test-passphrase"),
                passphrase_confirmation: SecretString::new("startup-test-passphrase"),
            }))
            .await
            .unwrap();
        engine.shutdown(Duration::from_secs(15)).await.unwrap();
    });

    let lease_path = root.path().join("blocked-closer.lease");
    let lease = File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let (release, released) = mpsc::channel();
    let entered = Arc::new(Notify::new());
    let (close_entered, close_wait) = mpsc::channel();
    let blocked_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(HeldSecureStorage {
            storage,
            entered: Arc::clone(&entered),
            release: Arc::new(Mutex::new(Some(released))),
        }),
        Box::new(Clipboard(Mutex::new(Some(Box::new(BlockedCloser {
            _lease: lease,
            entered: close_entered,
        }))))),
        Box::new(EmptyFiles),
    );
    let (input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        blocked_host,
        input,
        lifecycle_input,
    ));
    runtime.block_on(async {
        timeout(Duration::from_secs(15), entered.notified())
            .await
            .unwrap();
    });
    caller.abort();
    release.send(()).unwrap();
    close_wait.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(!available(&lease_path), "cleanup still owns the lock");
    drop(runtime);
    let snapshot = progress.snapshot();
    let retained = !available(&lease_path);
    drop(progress);
    drop(lifecycle);
    let retained_without_observer = !available(&lease_path);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("abandoned-startup-executor-stopped.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "real Engine and file lock, controlled blocked closer",
                "state": format!("{:?}", snapshot.state),
                "retry_allowed": snapshot.allowed_actions.retry,
                "retained_after_executor_stop": retained,
                "retained_after_observer_drop": retained_without_observer
            }))
            .unwrap(),
        )
        .unwrap();
    }
    assert!(
        !snapshot.allowed_actions.retry,
        "unconfirmed cleanup cannot grant startup retry"
    );
    assert!(
        retained,
        "stopped executor must not discard its live closer"
    );
    assert!(
        retained_without_observer,
        "closer must survive loss of its observers"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_startup_retains_failed_closer() {
    if run_isolated("startup::abandoned_cleanup::abandoned_startup_retains_failed_closer") {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("startup failure test".into()),
            passphrase: SecretString::new("startup-test-passphrase"),
            passphrase_confirmation: SecretString::new("startup-test-passphrase"),
        }))
        .await
        .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let lease_path = root.path().join("failed-closer.lease");
    let lease = File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let (release, released) = mpsc::channel();
    let entered = Arc::new(Notify::new());
    let blocked_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(HeldSecureStorage {
            storage,
            entered: Arc::clone(&entered),
            release: Arc::new(Mutex::new(Some(released))),
        }),
        Box::new(Clipboard(Mutex::new(Some(Box::new(FailedCloser(lease)))))),
        Box::new(EmptyFiles),
    );
    let (input, mut progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let caller = tokio::spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        blocked_host,
        input,
        lifecycle_input,
    ));
    timeout(Duration::from_secs(15), entered.notified())
        .await
        .unwrap();
    caller.abort();
    release.send(()).unwrap();
    assert!(caller.await.err().unwrap().is_cancelled());
    timeout(Duration::from_secs(20), async {
        while !progress.snapshot().state.is_terminal() {
            progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let snapshot = progress.snapshot();
    let retained = !available(&lease_path);
    drop(progress);
    drop(lifecycle);
    let retained_after_observer_drop = !available(&lease_path);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("abandoned-startup-failed-closer.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope":"real Engine and file lock, controlled host closer failure",
                "startup_state": format!("{:?}", snapshot.state),
                "retry_allowed": snapshot.allowed_actions.retry,
                "failed_closer_lock_retained": retained,
                "retained_after_observer_drop": retained_after_observer_drop
            }))
            .unwrap(),
        )
        .unwrap();
    }
    assert_eq!(snapshot.state, StartupState::Failed);
    assert!(
        !snapshot.allowed_actions.retry,
        "failed cleanup cannot grant startup retry"
    );
    assert!(retained, "failed cleanup must retain the actual closer");
    assert!(
        retained_after_observer_drop,
        "failed cleanup must remain owned after the observer is dropped"
    );
}
