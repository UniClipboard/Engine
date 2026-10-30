use std::fs::File;
use std::future::{poll_fn, Future};
use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use uc_engine::{
    Engine, EngineConfig, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardChange, HostClipboardChangeStream, HostClipboardSnapshot,
    HostDirectories, HostSecureStorage, StartupLifecycle, StartupProgress, StartupState,
};

use async_trait::async_trait;

use super::{host, EmptyClipboard, EmptyFiles, MemorySecureStorage};

#[cfg(feature = "test-binding-worker-panic")]
struct RetryableProbeStream {
    lease: Option<File>,
    calls: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(feature = "test-binding-worker-panic")]
#[async_trait]
impl HostClipboardChangeStream for RetryableProbeStream {
    fn supports_shutdown_retry(&self) -> bool {
        true
    }
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "controlled close failure",
            ));
        }
        self.release.notified().await;
        self.lease.take();
        Ok(())
    }
}

#[cfg(feature = "test-binding-worker-panic")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_startup_retry_joins_the_same_host_close() {
    use uc_engine::{HostConfigurationFile, HostConfigurationFileReset, StartupProbeOutcome};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("retry.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            RetryableProbeStream {
                lease: Some(lease),
                calls: Arc::clone(&calls),
                release: Arc::clone(&release),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let (progress, _) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    let config =
        EngineConfig::new("2.0.0").with_host_configuration_files([HostConfigurationFile::new(
            "../invalid.json",
            HostConfigurationFileReset::ByEngine,
        )]);
    let primary = match Engine::start_with_lifecycle(config, host, progress, input).await {
        Err(error) => error,
        Ok(_) => panic!("invalid host configuration accepted"),
    };
    let failed = lifecycle.probe_startup_outcome(Duration::from_secs(5));
    let initial_cleanup = match failed {
        StartupProbeOutcome::Failed(Some(ref error), ref cleanup, _, true) if *error == primary => {
            cleanup.clone()
        }
        _ => panic!("unexpected initial result"),
    };
    let probe = |path: &std::path::Path| {
        Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().unwrap().success()
    };
    assert!(!probe(&path));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        lifecycle.probe_retry_startup_outcome(Duration::ZERO),
        StartupProbeOutcome::Pending(_)
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) != 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!probe(&path));
    assert!(matches!(
        lifecycle.probe_retry_startup_outcome(Duration::ZERO),
        StartupProbeOutcome::Pending(_)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    release.notify_one();
    let released = lifecycle.probe_startup_outcome(Duration::from_secs(20));
    assert!(
        matches!(released, StartupProbeOutcome::Released(Some(ref error), Some(ref operation)) if *error == primary && *operation == initial_cleanup)
    );
    assert!(
        matches!(lifecycle.probe_startup_outcome(Duration::ZERO), StartupProbeOutcome::Released(Some(ref error), Some(ref operation)) if *error == primary && *operation == initial_cleanup)
    );
    assert!(probe(&path));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("engine-startup-explicit-retry.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine invalid configuration and controlled retryable host closer", "initial_failed":true, "host_lock_held_until_retry":true, "pending_joined":true, "host_close_calls":calls.load(Ordering::SeqCst), "startup_error_preserved":true, "prior_cleanup_error_preserved":true, "host_lock_released":true, "native_mobile":"not established"})).unwrap()).unwrap();
    }
}

struct FailedStream {
    _lease: File,
    calls: Arc<AtomicUsize>,
}

struct SuccessfulStream {
    lease: Option<File>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl HostClipboardChangeStream for SuccessfulStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.lease.take();
        Ok(())
    }
}

struct StageFailureStorage {
    storage: MemorySecureStorage,
    progress: StartupProgress,
    triggered: Arc<AtomicBool>,
}

impl HostSecureStorage for StageFailureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        if self.progress.snapshot().state == StartupState::StartingServices
            && !self.triggered.swap(true, Ordering::SeqCst)
        {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Unavailable,
                "controlled profile startup failure",
            ));
        }
        self.storage.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.storage.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.storage.delete(key)
    }
}

/// 失败清单：启动失败后遗漏资料停止器，宿主锁一直被持有，重试无法开始。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_preparation_failure_runs_cleanup() {
    const CHILD: &str = "UC_TEST_PROFILE_CLEANUP_ORDER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::profile_preparation_failure_runs_cleanup",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("profile-order.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let triggered = Arc::new(AtomicBool::new(false));
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, _control) = StartupLifecycle::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(StageFailureStorage {
            storage: MemorySecureStorage::default(),
            progress: progress.clone(),
            triggered: Arc::clone(&triggered),
        }),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            SuccessfulStream {
                lease: Some(lease),
                calls: Arc::clone(&calls),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let result =
        Engine::start_with_lifecycle(EngineConfig::new("2.0.0"), host, input, lifecycle).await;
    let status = Command::new("python3")
        .args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(matches!(status.code(), Some(0 | 1)));
    let held = !status.success();
    let close_calls = calls.load(Ordering::SeqCst);
    let retry_allowed = progress.snapshot().allowed_actions.retry;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("profile-cleanup-order.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope":"real Engine, controlled secure-storage failure and host closer, independent file lock probe",
                "stage_failure_triggered":triggered.load(Ordering::SeqCst),
                "host_close_calls":close_calls, "host_lock_retained":held,
                "retry_allowed":retry_allowed
            })).unwrap(),
        ).unwrap();
    }
    assert!(
        result.is_err(),
        "controlled storage failure must reject startup"
    );
    assert!(
        triggered.load(Ordering::SeqCst),
        "failure must occur during service preparation"
    );
    assert_eq!(close_calls, 1, "host closer must run after profile cleanup");
    assert!(!held, "confirmed cleanup must release the host lock");
}

#[async_trait]
impl HostClipboardChangeStream for FailedStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(HostCapabilityError::new(
            HostCapabilityErrorCategory::Io,
            "controlled startup close failure",
        ))
    }
}

struct FailedStreamClipboard(Mutex<Option<Box<dyn HostClipboardChangeStream>>>);

impl HostClipboard for FailedStreamClipboard {
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

struct BlockedStream {
    _lease: File,
    entered: mpsc::Sender<()>,
}

#[async_trait]
impl HostClipboardChangeStream for BlockedStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        let _ = self.entered.send(());
        std::future::pending().await
    }
}

/// 失败清单：回滚中执行器停止时，必须保留启动主错误和仍持有的宿主租约。
#[test]
fn failed_start_keeps_primary_error_when_executor_stops_during_host_close() {
    const CHILD: &str = "UC_TEST_FAILED_START_EXECUTOR_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::failed_start_keeps_primary_error_when_executor_stops_during_host_close",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let ipv6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).unwrap();
    let port = ipv6.local_addr().unwrap().port();
    let ipv4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let primary = runtime
        .block_on(Engine::start(
            EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
            host(
                &root.path().join("reference"),
                Box::new(MemorySecureStorage::default()),
            ),
        ))
        .err()
        .expect("occupied port rejects startup");
    let path = root.path().join("pending-start-closer.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let (entered, waiting) = mpsc::channel();
    let base = root.path().join("attempt");
    let host = HostCapabilities::new(
        HostDirectories::new(
            base.join("private"),
            base.join("cache"),
            base.join("temporary"),
            base.join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            BlockedStream {
                _lease: lease,
                entered,
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let _caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host,
        progress_input,
        lifecycle_input,
    ));
    waiting.recv_timeout(Duration::from_secs(20)).unwrap();
    drop(runtime);
    let status = Command::new("python3")
        .args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(matches!(status.code(), Some(0 | 1)));
    let retained = !status.success();
    let observer = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let observed = observer
        .block_on(lifecycle.suspend_with_deadline(Duration::from_secs(1)))
        .unwrap_err();
    let retry_allowed = progress.snapshot().allowed_actions.retry;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("partial-start-executor-stopped.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope":"real Engine and port conflict, controlled blocked host closer, independent file lock probe",
                "primary_code":primary.code(), "observed_code":observed.code(),
                "lock_retained":retained, "retry_allowed":retry_allowed
            })).unwrap(),
        ).unwrap();
    }
    drop(ipv4);
    drop(ipv6);
    assert!(retained, "pending host closer must remain owned");
    assert!(!retry_allowed, "unconfirmed cleanup cannot allow retry");
    assert_eq!(
        observed.code(),
        primary.code(),
        "startup primary error must survive executor loss"
    );
}

/// 失败清单：服务启动失败后必须关闭宿主变化流；关闭失败时不能丢弃租约或授权重试。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_service_start_retains_failed_host_closer() {
    const CHILD: &str = "UC_TEST_FAILED_START_CLOSER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::failed_service_start_retains_failed_host_closer",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("startup-closer.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let ipv6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).unwrap();
    let port = ipv6.local_addr().unwrap().port();
    let ipv4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            FailedStream {
                _lease: lease,
                calls: Arc::clone(&calls),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, _control) = StartupLifecycle::channel();
    let result = Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host,
        input,
        lifecycle,
    )
    .await;
    let observed = Command::new("python3")
        .args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(matches!(observed.code(), Some(0 | 1)));
    let retained = !observed.success();
    let attempts = calls.load(Ordering::SeqCst);
    let retry = progress.snapshot().allowed_actions.retry;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("partial-start-failed-closer.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope":"real Engine and port conflict, controlled host closer failure, independent file lock probe",
                "startup_failed":result.is_err(),
                "close_calls":attempts,
                "lock_retained":retained,
                "retry_allowed":retry
            }))
            .unwrap(),
        )
        .unwrap();
    }
    drop(ipv4);
    drop(ipv6);
    assert!(result.is_err(), "occupied port must reject startup");
    assert!(attempts > 0, "startup failure must invoke the host closer");
    assert!(retained, "failed host closer must remain owned");
    assert!(!retry, "unconfirmed startup cleanup must prevent retry");
}

struct ObservedStorage {
    storage: MemorySecureStorage,
    dropped: Arc<AtomicBool>,
}

impl Drop for ObservedStorage {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl HostSecureStorage for ObservedStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        self.storage.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.storage.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.storage.delete(key)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_service_start_releases_every_host_owner_before_returning() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let ipv6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).unwrap();
    let port = ipv6.local_addr().unwrap().port();
    let ipv4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok();
    let (progress, _) = StartupProgress::channel();
    let (lifecycle, control) = StartupLifecycle::channel();
    let mut pause = Box::pin(control.suspend());
    poll_fn(|context| {
        assert!(pause.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let result = Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host(
            root.path(),
            Box::new(ObservedStorage {
                storage: storage.clone(),
                dropped: Arc::clone(&dropped),
            }),
        ),
        progress,
        lifecycle,
    )
    .await;
    assert!(result.is_err(), "occupied network port must reject startup");
    assert_eq!(
        pause.await.unwrap_err().category(),
        result.err().unwrap().category()
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "failed startup retained a host owner"
    );
    drop(ipv4);
    drop(ipv6);
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

/// 未轮询的启动必须保留宿主 closer，且不能把直接丢弃当作清理成功。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpolled_startup_retains_host_closer() {
    const CHILD: &str = "UC_TEST_UNPOLLED_STARTUP_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::unpolled_startup_retains_host_closer",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("unpolled-startup.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, _control) = StartupLifecycle::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            FailedStream {
                _lease: lease,
                calls: Arc::clone(&calls),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let future = Engine::start_with_lifecycle(EngineConfig::new("2.0.0"), host, input, lifecycle);
    drop(future);
    let started = std::time::Instant::now();
    while calls.load(Ordering::SeqCst) == 0 && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = Command::new("python3")
        .args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&path).status().unwrap();
    assert!(matches!(status.code(), Some(0 | 1)));
    let held = !status.success();
    let retry = progress.snapshot().allowed_actions.retry;
    let close_calls = calls.load(Ordering::SeqCst);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("unpolled-startup.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"unpolled real Engine future, controlled host closer and independent lock probe", "host_lock_retained":held, "close_calls":close_calls, "retry_allowed":retry, "startup_release_receipt":"not established", "device_acceptance":"not established"})).unwrap()).unwrap();
    }
    assert!(held, "unconfirmed closer must remain owned");
    assert_eq!(close_calls, 1);
    assert!(!retry, "unconfirmed cleanup must prevent retry");
}

struct GatedUnpolledCloser {
    lease: Option<File>,
    entered: mpsc::Sender<()>,
    finished: mpsc::Sender<()>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl HostClipboardChangeStream for GatedUnpolledCloser {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.entered.send(()).unwrap();
        self.release.notified().await;
        self.lease.take();
        self.finished.send(()).unwrap();
        Ok(())
    }
}

/// 未轮询启动的清理独立于原执行器，并在宿主确认前继续持有锁。
#[test]
fn unpolled_startup_runs_owned_cleanup() {
    const CHILD: &str = "UC_TEST_UNPOLLED_STARTUP_CLEANUP_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::unpolled_startup_runs_owned_cleanup",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("unpolled-cleanup.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let (entered, waiting) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, _control) = StartupLifecycle::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            GatedUnpolledCloser {
                lease: Some(lease),
                entered,
                finished,
                release: Arc::clone(&release),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    drop(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        input,
        lifecycle,
    ));
    let started = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    let probe = || {
        Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap().success()
    };
    let held_while_pending = !probe();
    let retry_while_pending = progress.snapshot().allowed_actions.retry;
    release.notify_one();
    let finished = done.recv_timeout(Duration::from_secs(2)).is_ok();
    let released = probe();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("unpolled-startup-cleanup.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"unpolled real Engine future, controlled blocked host closer and independent lock probe", "cleanup_started":started, "host_lock_retained_while_pending":held_while_pending, "retry_allowed_while_pending":retry_while_pending, "cleanup_finished":finished, "host_lock_released":released, "startup_release_receipt":"not established", "device_acceptance":"not established"})).unwrap()).unwrap();
    }
    assert!(
        started && held_while_pending && !retry_while_pending && finished && released,
        "unpolled cleanup must be owned through close confirmation"
    );
}

/// 启动任务无法创建时，宿主清理不能随创建失败而丢失。
#[test]
fn startup_without_executor_returns_error_and_runs_cleanup() {
    const CHILD: &str = "UC_TEST_STARTUP_SPAWN_PANIC_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::startup_without_executor_returns_error_and_runs_cleanup",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("spawn-panic.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let (entered, waiting) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, _control) = StartupLifecycle::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            GatedUnpolledCloser {
                lease: Some(lease),
                entered,
                finished,
                release: Arc::clone(&release),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let mut future = Box::pin(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        input,
        lifecycle,
    ));
    let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        future.as_mut().poll(&mut context)
    }));
    let panic = polled.is_err();
    let stable_error = matches!(polled, Ok(Poll::Ready(Err(ref error))) if error.code() == 1108);
    drop(future);
    let started = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    let probe = || {
        Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap().success()
    };
    let held = !probe();
    let retry = progress.snapshot().allowed_actions.retry;
    release.notify_one();
    let finished = done.recv_timeout(Duration::from_secs(2)).is_ok();
    let released = probe();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("startup-no-executor-cleanup.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine startup polled without Tokio executor, controlled host closer and independent lock probe", "task_creation_panicked":panic, "stable_error":stable_error, "cleanup_started":started, "host_lock_retained_while_pending":held, "retry_allowed_while_pending":retry, "cleanup_finished":finished, "host_lock_released":released, "startup_release_receipt":"not established", "device_acceptance":"not established"})).unwrap()).unwrap();
    }
    assert!(
        !panic && stable_error && started && held && !retry && finished && released,
        "task creation failure must retain and run host cleanup"
    );
}

/// 已接收但从未轮询的启动任务仍须接管宿主清理。
#[test]
fn unpolled_startup_task_runs_owned_cleanup_after_executor_stops() {
    const CHILD: &str = "UC_TEST_UNPOLLED_STARTUP_TASK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::unpolled_startup_task_runs_owned_cleanup_after_executor_stops",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("unpolled-task.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let (entered, waiting) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, _control) = StartupLifecycle::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            GatedUnpolledCloser {
                lease: Some(lease),
                entered,
                finished,
                release: Arc::clone(&release),
            },
        ))))),
        Box::new(EmptyFiles),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut future = Box::pin(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        input,
        lifecycle,
    ));
    let accepted = {
        let _entered = runtime.enter();
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        matches!(future.as_mut().poll(&mut context), Poll::Pending)
    };
    drop(future);
    runtime.shutdown_timeout(Duration::from_millis(100));
    let started = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    let probe = || {
        Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap().success()
    };
    let held = !probe();
    let retry = progress.snapshot().allowed_actions.retry;
    release.notify_one();
    let finished = done.recv_timeout(Duration::from_secs(2)).is_ok();
    let released = probe();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("unpolled-startup-task.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine task accepted by a controlled executor that stops before polling, controlled host closer and independent lock probe", "task_accepted":accepted, "cleanup_started":started, "host_lock_retained_while_pending":held, "retry_allowed_while_pending":retry, "cleanup_finished":finished, "host_lock_released":released, "startup_release_receipt":"not established", "device_acceptance":"not established"})).unwrap()).unwrap();
    }
    assert!(
        accepted && started && held && !retry && finished && released,
        "unpolled accepted task must run owned host cleanup"
    );
}

struct PanicUnpolledCloser {
    _lease: File,
    entered: mpsc::Sender<()>,
}

#[async_trait]
impl HostClipboardChangeStream for PanicUnpolledCloser {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.entered.send(()).unwrap();
        panic!("controlled host close panic");
    }
}

/// 清理线程中的宿主回调异常不能丢弃原 closer 或开放重试。
#[test]
fn unpolled_cleanup_panic_retains_host_closer() {
    const CHILD: &str = "UC_TEST_UNPOLLED_CLEANUP_PANIC_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::failure::unpolled_cleanup_panic_retains_host_closer",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cleanup-panic.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let (entered, waiting) = mpsc::channel();
    let (input, progress) = StartupProgress::channel();
    let (lifecycle, control) = StartupLifecycle::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedStreamClipboard(Mutex::new(Some(Box::new(
            PanicUnpolledCloser {
                _lease: lease,
                entered,
            },
        ))))),
        Box::new(EmptyFiles),
    );
    drop(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        input,
        lifecycle,
    ));
    let called = waiting.recv_timeout(Duration::from_secs(5)).is_ok();
    drop(control);
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap();
    assert!(matches!(status.code(), Some(0 | 1)));
    let held = !status.success();
    let retry = progress.snapshot().allowed_actions.retry;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("unpolled-cleanup-panic.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"unpolled real Engine future, panicking controlled host closer, independent lock probe", "close_callback_entered":called, "host_lock_retained":held, "retry_allowed":retry, "terminal_result_observed":"not established", "device_acceptance":"not established"})).unwrap()).unwrap();
    }
    assert!(
        called && held && !retry,
        "panic must retain the unconfirmed host closer"
    );
}
