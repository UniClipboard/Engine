//! 使用真实启动负责人和受控宿主关闭验证私有结果；不代表设备验收。
#![cfg(not(test))]
#![allow(dead_code)]
#[path = "../src/assembly/mod.rs"]
mod assembly;
#[path = "../src/compatibility/mod.rs"]
mod compatibility;
#[path = "../src/contract/mod.rs"]
mod contract;
#[cfg(feature = "dev-tools")]
#[path = "../src/dev/mod.rs"]
mod dev;
#[path = "../src/engine/mod.rs"]
mod engine;
#[path = "../src/operations/mod.rs"]
mod operations;
#[path = "../src/runtime/mod.rs"]
mod runtime;
#[path = "../src/subsystems/mod.rs"]
mod subsystems;
pub use compatibility::mobile_lan::*;
pub use contract::*;
#[cfg(feature = "dev-tools")]
pub use dev::*;
pub use engine::{
    Engine, EventStream, StartupLifecycle, StartupLifecycleInput, StartupProgress,
    StartupProgressInput,
};

use async_trait::async_trait;
use engine::StartupOutcomeSnapshot;
use std::collections::HashMap;
use std::fs::File;
use std::future::Future;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

#[derive(Default)]
struct Storage(Mutex<HashMap<String, Vec<u8>>>);
impl HostSecureStorage for Storage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.0.lock().expect("storage").get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.0
            .lock()
            .expect("storage")
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.0.lock().expect("storage").remove(key);
        Ok(())
    }
}
struct BackupStorage {
    values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    backup_root: std::path::PathBuf,
    fail_after_release: bool,
}
impl HostSecureStorage for BackupStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        if key.starts_with("kek:v1:") && self.backup_root.exists() {
            let gate = self.gate.lock().expect("gate").take();
            if let Some((entered, release)) = gate {
                entered.send(()).expect("entered");
                release
                    .recv_timeout(Duration::from_secs(20))
                    .expect("release backup");
                if self.fail_after_release {
                    return Err(HostCapabilityError::new(
                        HostCapabilityErrorCategory::Unavailable,
                        "controlled late storage failure",
                    ));
                }
            }
        }
        Ok(self.values.lock().expect("storage").get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.values
            .lock()
            .expect("storage")
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.values.lock().expect("storage").remove(key);
        Ok(())
    }
}

struct CoreGateStorage {
    panic_after_release: bool,
    lease: Mutex<Option<File>>,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl uc_core::ports::SecureStoragePort for CoreGateStorage {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>, uc_core::ports::SecureStorageError> {
        Ok(None)
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), uc_core::ports::SecureStorageError> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), uc_core::ports::SecureStorageError> {
        self.entered.send(()).expect("backup entered");
        self.release
            .lock()
            .expect("release lock")
            .recv()
            .expect("backup release");
        self.lease.lock().expect("lease lock").take();
        if self.panic_after_release {
            panic!("controlled backup panic");
        }
        Err(uc_core::ports::SecureStorageError::Unavailable(
            "controlled backup error".into(),
        ))
    }
}

struct Files;
impl HostFileAccess for Files {
    fn metadata(&self, _: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Err(unavailable())
    }
    fn read_chunk(
        &self,
        _: &HostFileHandle,
        _: u64,
        _: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Err(unavailable())
    }
    fn write_chunk(&self, _: &HostFileHandle, _: u64, _: &[u8]) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}
fn unavailable() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "controlled closer failure",
    )
}
struct Stream {
    lease: Option<File>,
    entered: mpsc::Sender<()>,
    release: Arc<tokio::sync::Notify>,
    fail: bool,
}
#[async_trait]
impl HostClipboardChangeStream for Stream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.entered.send(()).expect("entered");
        self.release.notified().await;
        if self.fail {
            return Err(unavailable());
        }
        self.lease.take();
        Ok(())
    }
}
struct RetryStream {
    panic_on_close: bool,
    lease: Option<File>,
    entered: mpsc::Sender<()>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl HostClipboardChangeStream for RetryStream {
    fn supports_shutdown_retry(&self) -> bool {
        true
    }
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.send(()).expect("entered");
        if self.panic_on_close {
            panic!("controlled close panic");
        }
        self.release.notified().await;
        if call < 2 {
            return Err(unavailable());
        }
        self.lease.take();
        Ok(())
    }
}
struct RetryClipboard(Mutex<Option<RetryStream>>);
impl HostClipboard for RetryClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .lock()
            .expect("clipboard")
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

struct Clipboard(Mutex<Option<Stream>>);
impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .lock()
            .expect("clipboard")
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}
fn available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}
fn unpolled_progress_after_cleanup(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("unpolled.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(Stream {
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            fail: false,
        })))),
        Box::new(Files),
    );
    let (progress, observer) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let startup = Engine::start_with_lifecycle(EngineConfig::new("2.0.0"), host, progress, input);
    drop(startup);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("close started");
    let pending_retry = observer.snapshot().allowed_actions.retry;
    let held = !available(&path);
    release.notify_one();
    let result = control.wait_release_internal(Duration::from_secs(5));
    let until = std::time::Instant::now() + Duration::from_secs(1);
    while !observer.snapshot().allowed_actions.retry && std::time::Instant::now() < until {
        std::thread::yield_now();
    }
    let snapshot = observer.snapshot();
    let free = available(&path);
    let artifact = serde_json::json!({"scope":"real unpolled Engine startup with controlled host closer and independent lock", "retry_while_pending":pending_retry, "lock_held":held, "result":format!("{result:?}"), "progress_state":format!("{:?}", snapshot.state), "retry_after_release":snapshot.allowed_actions.retry, "lock_available":free});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-unpolled-progress.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        !pending_retry
            && held
            && free
            && matches!(result, StartupOutcomeSnapshot::Released { .. })
            && snapshot.allowed_actions.retry
    );
    artifact
}

fn host_close_retry(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("retry.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let (progress, progress_observer) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let mut startup = Box::pin(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    let mut context = Context::from_waker(Waker::noop());
    let primary = match startup.as_mut().poll(&mut context) {
        Poll::Ready(Err(error)) => error,
        _ => panic!("expected no-executor error"),
    };
    drop(startup);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("first close");
    release.notify_one();
    let failed = control.wait_release_internal(Duration::from_secs(5));
    let stable_failure = failed == control.wait_release_internal(Duration::ZERO);
    let retry_advertised = matches!(
        failed,
        StartupOutcomeSnapshot::Failed {
            explicit_retry_supported: true,
            ..
        }
    );
    let lock_retained = !available(&path);
    let calls_before_retry = calls.load(std::sync::atomic::Ordering::SeqCst);
    let retry_pending = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("second close");
    let joined_retry = control.retry_host_internal(Duration::ZERO);
    let joining = control.clone();
    let waiter = std::thread::spawn(move || joining.wait_release_internal(Duration::from_secs(10)));
    let still_pending = matches!(retry_pending, StartupOutcomeSnapshot::Cleaning { .. })
        && joined_retry == retry_pending
        && !waiter.is_finished()
        && !available(&path);
    release.notify_one();
    let second_failure = waiter.join().expect("waiter");
    let failed_twice = matches!(
        second_failure,
        StartupOutcomeSnapshot::Failed {
            explicit_retry_supported: true,
            ..
        }
    ) && !available(&path)
        && calls.load(std::sync::atomic::Ordering::SeqCst) == 2;
    let third_pending = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("third close");
    let held_during_retry = !available(&path);
    let progress_retry_during = progress_observer.snapshot().allowed_actions.retry;
    release.notify_one();
    let result = control.wait_release_internal(Duration::from_secs(5));
    let repeated = control.wait_release_internal(Duration::ZERO);
    let released = matches!(result, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(_) } if *error == primary);
    let lock_released = available(&path);
    let calls_final = calls.load(std::sync::atomic::Ordering::SeqCst);
    let limit = std::time::Instant::now() + Duration::from_secs(1);
    while !progress_observer.snapshot().allowed_actions.retry && std::time::Instant::now() < limit {
        std::thread::yield_now();
    }
    let progress_retry_after = progress_observer.snapshot().allowed_actions.retry;
    let artifact = serde_json::json!({"scope":"real Engine no-executor startup, retained host stream and independent lock", "primary":format!("{primary:?}"), "failed":format!("{failed:?}"), "stable_failure":stable_failure, "retry_advertised":retry_advertised, "lock_retained":lock_retained, "calls_before_retry":calls_before_retry, "retry_pending":format!("{retry_pending:?}"), "concurrent_wait_pending":still_pending, "second_failure":format!("{second_failure:?}"), "failed_twice":failed_twice, "third_pending":format!("{third_pending:?}"), "held_during_retry":held_during_retry, "progress_retry_during":progress_retry_during, "progress_retry_after":progress_retry_after, "result":format!("{result:?}"), "retry_released":released, "lock_released":lock_released, "repeated_equal":result==repeated, "calls_final":calls_final});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-host-retry.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        stable_failure
            && retry_advertised
            && lock_retained
            && calls_before_retry == 1
            && still_pending
            && failed_twice
            && matches!(third_pending, StartupOutcomeSnapshot::Cleaning { .. })
            && held_during_retry
            && !progress_retry_during
            && progress_retry_after
            && released
            && lock_released
            && result == repeated
            && calls_final == 3
    );
    artifact
}

fn host_close_with_active_backup(root: &Path, panic_after_release: bool) -> serde_json::Value {
    use uc_application::deps::RetireUpgradeBackupSecurityRecordsPort;
    use uc_infra_profile::security::ProfileUpgradeBackupStore;
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("backup.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let directories = HostDirectories::new(
        root.join("private"),
        root.join("cache"),
        root.join("temporary"),
        root.join("logs"),
    );
    let paths = assembly::host::derive_app_paths(&directories);
    let storage = Arc::new(CoreGateStorage {
        panic_after_release,
        lease: Mutex::new(Some(lease)),
        entered,
        release: Mutex::new(released),
    });
    let backup =
        ProfileUpgradeBackupStore::new(paths, "controlled".into(), storage, root.join("backups"));
    let owner = Arc::new(engine::startup_owner::StartupCleanupOwner::default());
    owner.register_backup_activity(backup.activity());
    let host = Arc::new(runtime::ReusableHost::new(HostCapabilities::new(
        directories,
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(None))),
        Box::new(Files),
    )));
    owner.accept_host(Arc::clone(&host));
    let primary = EngineError::new(1101, EngineErrorCategory::Unavailable, true);
    let (progress_input, progress) = StartupProgress::channel();
    owner.record_primary(primary.clone());
    owner.begin_cleanup();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let work = runtime.spawn(async move { backup.retire_upgrade_backup_security_records().await });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("backup entered");
    runtime
        .block_on(host.close_change_stream())
        .expect("host close");
    owner.host_closed();
    owner.finish_construction_failure(
        EngineError::new(1108, EngineErrorCategory::Internal, false),
        progress_input,
        primary.clone(),
    );
    let pending = owner.wait_internal(Duration::ZERO);
    let progress_retry_while_pending = progress.snapshot().allowed_actions.retry;
    let held = !available(&path);
    release.send(()).expect("release backup");
    let failed = runtime.block_on(work).expect("backup task").is_err();
    let result = owner.wait_internal(Duration::from_secs(5));
    let repeated = owner.wait_internal(Duration::ZERO);
    let free = available(&path);
    let complete = if panic_after_release {
        matches!(result, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), operation_error: Some(_), explicit_retry_supported: false, .. } if *error == primary)
    } else {
        matches!(result, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(_) } if *error == primary)
    };
    let progress_retry = progress.snapshot().allowed_actions.retry;
    let progress_matches_release = progress_retry == !panic_after_release;
    let artifact = serde_json::json!({"progress_retry_while_pending":progress_retry_while_pending, "progress_retry":progress_retry, "progress_matches_release":progress_matches_release, "scope":"production startup owner and real backup activity with controlled storage and independent lock", "panic_after_release":panic_after_release, "pending":format!("{pending:?}"), "lock_held":held, "backup_failed":failed, "result":format!("{result:?}"), "expected_terminal_result":complete, "repeated_equal":result==repeated, "lock_available":free, "full_startup_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join(if panic_after_release {
                "startup-construction-backup-panic.json"
            } else {
                "startup-construction-backup.json"
            }),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
            && !progress_retry_while_pending
            && held
            && failed
            && complete
            && progress_matches_release
            && result == repeated
            && free
    );
    artifact
}

fn host_close_before_construction_settles(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("construction.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = Arc::new(runtime::ReusableHost::new(HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(Stream {
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            fail: false,
        })))),
        Box::new(Files),
    )));
    let owner = Arc::new(engine::startup_owner::StartupCleanupOwner::default());
    owner.accept_host(Arc::clone(&host));
    let primary = EngineError::new(1109, EngineErrorCategory::InvalidInput, false);
    owner.record_primary(primary.clone());
    owner.begin_cleanup();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let closing_host = Arc::clone(&host);
    let closer = runtime.spawn(async move { closing_host.close_change_stream().await });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("host close");
    release.notify_one();
    runtime
        .block_on(closer)
        .expect("closer task")
        .expect("close");
    owner.host_closed();
    let before = owner.wait_internal(Duration::ZERO);
    let host_closed = available(&path);
    let pending = matches!(before, StartupOutcomeSnapshot::Cleaning { .. });
    let (progress_input, _) = StartupProgress::channel();
    owner.finish_construction_failure(
        EngineError::new(1108, EngineErrorCategory::Internal, false),
        progress_input,
        primary.clone(),
    );
    let after = owner.wait_internal(Duration::ZERO);
    let result = serde_json::json!({"scope":"production startup owner, controlled construction boundary, real host lock", "host_closed":host_closed, "before_construction_settled":format!("{before:?}"), "pending_before_settlement":pending, "after_construction_settled":format!("{after:?}"), "full_startup_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-construction-close.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        host_closed
            && pending
            && matches!(after, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), .. } if *error == primary)
    );
    result
}

fn host_close_retry_with_executor(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("retry.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let config =
        EngineConfig::new("2.0.0").with_host_configuration_files([HostConfigurationFile::new(
            "../invalid.json",
            HostConfigurationFileReset::ByEngine,
        )]);
    let (progress, progress_observer) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let task = runtime.spawn(Engine::start_with_lifecycle(config, host, progress, input));
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("first close");
    let pending = control.retry_host_internal(Duration::ZERO);
    let pending_held = matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
        && !available(&path)
        && calls.load(std::sync::atomic::Ordering::SeqCst) == 1;
    release.notify_one();
    let primary = match runtime.block_on(task).expect("task") {
        Err(error) => error,
        Ok(_) => panic!("invalid configuration accepted"),
    };
    let failed = control.wait_release_internal(Duration::from_secs(5));
    let retryable = matches!(failed, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), explicit_retry_supported: true, .. } if *error == primary);
    let retained = !available(&path);
    let second_pending = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("second close");
    let second_held = !available(&path);
    release.notify_one();
    let second_failed = control.wait_release_internal(Duration::from_secs(5));
    let second_retryable = matches!(second_failed, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), explicit_retry_supported: true, .. } if *error == primary)
        && !available(&path);
    let third_pending = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("third close");
    let third_held = !available(&path);
    release.notify_one();
    let released = control.wait_release_internal(Duration::from_secs(5));
    let repeated = control.wait_release_internal(Duration::ZERO);
    let closed = available(&path);
    let nonretryable_progress = !progress_observer.snapshot().allowed_actions.retry;
    let result = serde_json::json!({"scope":"real Engine invalid configuration with running executor and real host lock", "pending_held":pending_held, "primary":format!("{primary:?}"), "failed":format!("{failed:?}"), "retryable":retryable, "lock_retained":retained, "second_pending":format!("{second_pending:?}"), "second_held":second_held, "second_retryable":second_retryable, "third_pending":format!("{third_pending:?}"), "third_held":third_held, "released":format!("{released:?}"), "repeat_equal":released==repeated, "lock_closed":closed, "nonretryable_progress":nonretryable_progress});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-executor-host-retry.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        pending_held
            && retryable
            && retained
            && second_held
            && second_retryable
            && third_held
            && closed
            && nonretryable_progress
            && released == repeated
            && matches!(released, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(_) } if *error == primary)
    );
    result
}

#[cfg(feature = "dev-tools")]
fn host_retry_after_backup_registration(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("retry.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let socket = std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).expect("port");
    let port = socket.local_addr().expect("address").port();
    let ipv4 = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).ok();
    let config = EngineConfig::new("2.0.0").with_test_iroh_bind_port(port);
    let (progress, progress_observer) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let task = runtime.spawn(Engine::start_with_lifecycle(config, host, progress, input));
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("first close");
    let pending = control.retry_host_internal(Duration::ZERO);
    let pending_held = matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
        && !available(&path)
        && calls.load(std::sync::atomic::Ordering::SeqCst) == 1;
    release.notify_one();
    let primary = match runtime.block_on(task).expect("task") {
        Err(error) => error,
        Ok(_) => panic!("occupied port accepted"),
    };
    let failed = control.wait_release_internal(Duration::from_secs(5));
    let retryable = matches!(failed, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), explicit_retry_supported: true, .. } if *error == primary);
    let retained = !available(&path);
    if !retryable {
        let result = serde_json::json!({"failed":format!("{failed:?}"), "retryable":false, "lock_retained":retained});
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("directory");
            std::fs::write(
                Path::new(&directory).join("startup-registered-backup-host-retry.json"),
                serde_json::to_vec_pretty(&result).expect("json"),
            )
            .expect("artifact");
        }
        panic!("settled registered backup blocked host retry");
    }
    let second_pending = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("second close");
    let second_held = !available(&path);
    release.notify_one();
    let second_failed = control.wait_release_internal(Duration::from_secs(5));
    let second_retryable = matches!(second_failed, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), explicit_retry_supported: true, .. } if *error == primary)
        && !available(&path);
    let third_pending = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("third close");
    let third_held = !available(&path);
    release.notify_one();
    let released = control.wait_release_internal(Duration::from_secs(5));
    let repeated = control.wait_release_internal(Duration::ZERO);
    let closed = available(&path);
    let retryable_progress = progress_observer.snapshot().allowed_actions.retry;
    let result = serde_json::json!({"scope":"real Engine occupied port, registered backup activity and real host lock", "pending_held":pending_held, "primary":format!("{primary:?}"), "failed":format!("{failed:?}"), "retryable":retryable, "lock_retained":retained, "second_pending":format!("{second_pending:?}"), "second_held":second_held, "second_retryable":second_retryable, "third_pending":format!("{third_pending:?}"), "third_held":third_held, "released":format!("{released:?}"), "repeat_equal":released==repeated, "lock_closed":closed, "retryable_progress":retryable_progress});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-registered-backup-host-retry.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        pending_held
            && retryable
            && retained
            && second_held
            && second_retryable
            && third_held
            && closed
            && retryable_progress
            && released == repeated
            && matches!(released, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(_) } if *error == primary)
    );
    drop(ipv4);
    drop(socket);
    result
}

fn host_close_panic(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("panic.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: true,
            lease: Some(lease),
            entered,
            release: Arc::new(tokio::sync::Notify::new()),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let (progress, _) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let mut startup = Box::pin(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        startup.as_mut().poll(&mut context),
        Poll::Ready(Err(_))
    ));
    drop(startup);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("panic close entered");
    let failed = control.wait_release_internal(Duration::from_secs(5));
    let retried = control.retry_host_internal(Duration::ZERO);
    let held = !available(&path);
    let calls = calls.load(std::sync::atomic::Ordering::SeqCst);
    let terminal = matches!(
        failed,
        StartupOutcomeSnapshot::Failed {
            explicit_retry_supported: false,
            ..
        }
    );
    let result = serde_json::json!({"scope":"real no-executor startup; controlled host callback panic and independent lock", "failed":format!("{failed:?}"), "retry_equal":failed==retried, "lock_retained":held, "close_calls":calls, "terminal_retry_disabled":terminal});
    assert!(terminal && retried == failed && held && calls == 1);
    result
}

fn abandoned_input() -> serde_json::Value {
    let (input, control) = StartupLifecycle::channel();
    let before = control.wait_release_internal(Duration::ZERO);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let waiting_control = control.clone();
    let request = runtime.spawn(async move { waiting_control.suspend().await });
    runtime.block_on(tokio::task::yield_now());
    drop(input);
    let request_failed = runtime.block_on(request).expect("request task").is_err();
    let after = control.wait_release_internal(Duration::ZERO);
    let repeated = control.wait_release_internal(Duration::ZERO);
    let distinct = before != after;
    let result = serde_json::json!({"scope":"real startup lifecycle with unused input", "before":format!("{before:?}"), "after":format!("{after:?}"), "distinct":distinct, "repeat_equal":after==repeated, "pending_request_failed":request_failed, "full_startup_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-abandoned-input.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        distinct
            && matches!(after, StartupOutcomeSnapshot::InputDropped { .. })
            && after == repeated
            && request_failed
    );
    result
}

fn scenario(root: &Path, fail: bool) -> serde_json::Value {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(Stream {
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            fail,
        })))),
        Box::new(Files),
    );
    let (progress, _) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let initial = control.wait_release_internal(Duration::ZERO);
    let mut startup = Box::pin(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    let mut context = Context::from_waker(Waker::noop());
    let primary = match startup.as_mut().poll(&mut context) {
        Poll::Ready(Err(error)) => error,
        _ => panic!("expected no-executor startup error"),
    };
    drop(startup);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("closer started");
    let pending = control.wait_release_internal(Duration::ZERO);
    let held = !available(&path);
    release.notify_one();
    let terminal = control.wait_release_internal(Duration::from_secs(5));
    let repeated = control.wait_release_internal(Duration::ZERO);
    let held_after = !available(&path);
    assert!(matches!(initial, StartupOutcomeSnapshot::NotStarted));
    assert!(
        matches!(pending, StartupOutcomeSnapshot::Cleaning { startup_error: Some(ref error) } if *error == primary)
    );
    assert!(held && terminal == repeated);
    let unsupported_retry_preserved = if fail {
        let unsupported_retry = control.retry_host_internal(Duration::ZERO);
        assert_eq!(unsupported_retry, terminal);
        assert!(matches!(
            terminal,
            StartupOutcomeSnapshot::Failed {
                explicit_retry_supported: false,
                ..
            }
        ));
        assert!(held_after && !available(&path));
        assert!(
            matches!(terminal, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), .. } if *error == primary)
        );
        true
    } else {
        assert!(!held_after);
        assert!(
            matches!(terminal, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), .. } if *error == primary)
        );
        true
    };
    serde_json::json!({"unsupported_retry_preserved":unsupported_retry_preserved, "closer_failure_injected":fail, "initial":format!("{initial:?}"), "pending":format!("{pending:?}"), "lock_held_before_close":held, "terminal":format!("{terminal:?}"), "repeat_equal":terminal==repeated, "lock_held_after_close":held_after})
}
fn delivered_engine(root: &Path) -> serde_json::Value {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(None))),
        Box::new(Files),
    );
    let (input, _) = StartupProgress::channel();
    let (lifecycle, control) = StartupLifecycle::channel();
    let (engine, events) = runtime
        .block_on(Engine::start_with_lifecycle(
            EngineConfig::new("2.0.0"),
            host,
            input,
            lifecycle,
        ))
        .expect("start engine");
    let delivered = control.wait_release_internal(Duration::ZERO);
    let engine_release = runtime.block_on(engine.shutdown_until_released());
    let repeated = control.wait_release_internal(Duration::ZERO);
    drop(events);
    assert!(matches!(delivered, StartupOutcomeSnapshot::Delivered) && delivered == repeated);
    assert!(matches!(
        engine_release,
        ResourceReleaseOutcome::Released { .. }
    ));
    serde_json::json!({"scope":"real Engine start, delivery and per-Engine shutdown", "startup_after_delivery":format!("{delivered:?}"), "engine_release":format!("{engine_release:?}"), "startup_after_shutdown":format!("{repeated:?}"), "startup_result_unchanged":delivered==repeated})
}

fn executor_loss_during_backup(root: &Path, fail_after_release: bool) -> serde_json::Value {
    use std::time::Instant;
    let values = Arc::new(Mutex::new(HashMap::new()));
    let host = |gate| {
        HostCapabilities::new(
            HostDirectories::new(
                root.join("private"),
                root.join("cache"),
                root.join("temporary"),
                root.join("logs"),
            ),
            Box::new(BackupStorage {
                values: Arc::clone(&values),
                gate: Mutex::new(gate),
                backup_root: root.join("private-upgrade-backups"),
                fail_after_release,
            }),
            Box::new(Clipboard(Mutex::new(None))),
            Box::new(Files),
        )
    };
    let initial = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("initial runtime");
    initial.block_on(async {
        let (engine, _) = Engine::start(EngineConfig::new("1.0.0"), host(None))
            .await
            .expect("initial engine");
        engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("backup fixture".into()),
                passphrase: SecretString::new("fixture passphrase"),
                passphrase_confirmation: SecretString::new("fixture passphrase"),
            }))
            .await
            .expect("create fixture");
        engine
            .shutdown(Duration::from_secs(15))
            .await
            .expect("shutdown fixture");
    });
    drop(initial);
    std::fs::write(
        root.join("private/upgrade-cursor.json"),
        br#"{"schema_version":1,"last_seen_version":"1.0.0"}"#,
    )
    .expect("old version fixture");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (progress, observer) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host(Some((entered, released))),
        progress,
        input,
    ));
    if waiting.recv_timeout(Duration::from_secs(20)).is_err() {
        let status = observer.snapshot();
        let finished = caller.is_finished();
        panic!(
            "backup gate not reached; startup={:?}, caller_finished={}",
            status.state, finished
        );
    }
    let backups = root.join("private-upgrade-backups");
    let lease = std::fs::read_dir(&backups)
        .expect("backup root")
        .map(|item| item.expect("entry").path().join(".lease"))
        .find(|path| path.exists())
        .expect("backup lease");
    let held_before = !available(&lease);
    runtime.shutdown_timeout(Duration::from_millis(50));
    let after_shutdown = lifecycle.wait_release_internal(Duration::ZERO);
    let held_after = !available(&lease);
    release.send(()).expect("release backup");
    let until = Instant::now() + Duration::from_secs(20);
    while !available(&lease) {
        assert!(Instant::now() < until, "backup did not release its lease");
        std::thread::sleep(Duration::from_millis(10));
    }
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(5));
    let pending = matches!(
        after_shutdown,
        StartupOutcomeSnapshot::Constructing { .. } | StartupOutcomeSnapshot::Cleaning { .. }
    );
    let failure = matches!(terminal, StartupOutcomeSnapshot::Failed { .. });
    let released = matches!(terminal, StartupOutcomeSnapshot::Released { .. });
    let late_error_observed = format!("{terminal:?}").contains("operation_error: Some(");
    let result = serde_json::json!({"scope":"real Engine profile upgrade backup, controlled secure storage gate, independent backup lease probe", "lock_held_before_shutdown":held_before, "lock_held_after_shutdown":held_after, "result_after_shutdown":format!("{after_shutdown:?}"), "pending_while_backup_active":pending, "result_after_backup":format!("{terminal:?}"), "failed_after_backup":failure, "late_error_injected":fail_after_release, "late_error_observed":late_error_observed});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join("startup-backup-loss.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        held_before && held_after && pending && (released || (fail_after_release && failure)),
        "backup work must settle before its terminal startup result"
    );
    assert!(
        !fail_after_release || late_error_observed,
        "late backup failure must be retained independently"
    );
    result
}

fn executor_loss_during_registration(root: &Path) -> serde_json::Value {
    use std::time::Instant;
    use uc_observability_runtime::{
        DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig,
        NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
        ProcessObservabilityRuntime, SignalResult,
    };
    let log_root = root.join("process-logs");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_network_logs(
        NetworkLogConfig::new(
            &log_root,
            NetworkProcessRole::MobileApp,
            NetworkLogBudget {
                record_bytes: 1024,
                queue_records: 32,
                file_bytes: 4096,
                run_bytes: 8192,
                root_bytes: 16384,
                retention: Duration::from_secs(60),
            },
        )
        .expect("network"),
    );
    let observability = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut first, _) = EngineLogActivity::begin(Duration::from_secs(5));
    let lease = observability
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("initial export")
        .run_directory
        .join("lease.lock");
    assert_eq!(
        first.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let fifo = log_root.join("network/.cleanup.lock");
    std::fs::remove_file(&fifo).expect("remove lock");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo")
        .success());
    let (mut blocker, blocked_resume) = EngineLogActivity::begin(Duration::from_millis(50));
    let blocked_park = blocker.end(Duration::from_millis(50));
    let blocked_fifo = log_root.join("network/blocked.fifo");
    std::fs::rename(&fifo, &blocked_fifo).expect("rename fifo");
    std::fs::write(&fifo, []).expect("restore lock");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(None))),
        Box::new(Files),
    );
    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress_input,
        lifecycle_input,
    ));
    let until = Instant::now() + Duration::from_secs(20);
    while progress.snapshot().state != StartupState::StartingServices {
        assert!(
            Instant::now() < until && !caller.is_finished(),
            "startup did not reach services"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(!caller.is_finished(), "startup must remain blocked");
    runtime.shutdown_timeout(Duration::from_millis(50));
    let pending = lifecycle.wait_release_internal(Duration::ZERO);
    let (opened, waiting) = mpsc::channel();
    let (close, closing) = mpsc::channel();
    let fifo_reader = std::thread::spawn(move || {
        let reader = File::open(&blocked_fifo).expect("open fifo");
        opened.send(()).expect("opened");
        closing.recv().expect("close fifo");
        drop(reader);
    });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("fifo opened");
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(10));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("startup-runtime-cleanup-diagnostic.json"), serde_json::to_vec_pretty(&serde_json::json!({"pending":format!("{pending:?}"), "after_registration":format!("{terminal:?}"), "repeated":format!("{repeated:?}")})).expect("json")).expect("artifact");
    }
    let released_lease = available(&lease);
    let (mut other, _) = EngineLogActivity::begin(Duration::from_secs(5));
    let reopened = !available(&lease);
    let other_end = other.end(Duration::from_secs(5));
    let parked_again = available(&lease);
    close.send(()).expect("close");
    fifo_reader.join().expect("reader");
    assert_eq!(blocked_resume, Some(SignalResult::TimedOut));
    assert_eq!(blocked_park, Some(SignalResult::TimedOut));
    assert!(matches!(
        pending,
        StartupOutcomeSnapshot::Constructing { .. } | StartupOutcomeSnapshot::Cleaning { .. }
    ));
    assert!(matches!(terminal, StartupOutcomeSnapshot::Released { .. }) && terminal == repeated);
    assert!(
        released_lease && reopened && other_end == Some(SignalResult::Completed) && parked_again
    );
    serde_json::json!({"scope":"production Engine and network owner, controlled FIFO, independent lease probe", "pending":format!("{pending:?}"), "terminal":format!("{terminal:?}"), "repeat_equal":terminal==repeated, "network_lease_released":released_lease, "other_registration_reopened":reopened, "network_lease_parked_again":parked_again})
}

struct FailedProfileStopper {
    _lease: File,
}

#[async_trait]
impl uc_application::deps::StopProfileRuntimePort for FailedProfileStopper {
    async fn stop_profile_runtime(&self) -> Result<(), uc_application::deps::LifecycleError> {
        Err(uc_application::deps::LifecycleError {
            primary: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "controlled rollback failure",
            )
            .into(),
            additional: Vec::new(),
        })
    }
}

fn rollback_with_active_backup(
    root: &Path,
    panic_after_release: bool,
    settle_first: bool,
    fail_host: bool,
) -> serde_json::Value {
    use uc_application::deps::{RetireUpgradeBackupSecurityRecordsPort, StopProfileRuntimePort};
    use uc_infra_profile::security::ProfileUpgradeBackupStore;
    std::fs::create_dir_all(root).expect("root");
    let backup_path = root.join("backup.lease");
    let backup_lease = File::create(&backup_path).expect("backup lease");
    backup_lease.lock().expect("backup lock");
    let stopper_path = root.join("stopper.lease");
    let stopper_lease = File::create(&stopper_path).expect("stopper lease");
    stopper_lease.lock().expect("stopper lock");
    let host_path = root.join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let directories = HostDirectories::new(
        root.join("private"),
        root.join("cache"),
        root.join("temp"),
        root.join("logs"),
    );
    let storage = Arc::new(CoreGateStorage {
        panic_after_release,
        lease: Mutex::new(Some(backup_lease)),
        entered,
        release: Mutex::new(released),
    });
    let backup = ProfileUpgradeBackupStore::new(
        assembly::host::derive_app_paths(&directories),
        "controlled".into(),
        storage,
        root.join("backups"),
    );
    let owner = Arc::new(engine::startup_owner::StartupCleanupOwner::default());
    owner.register_backup_activity(backup.activity());
    let (close_entered, close_wait) = mpsc::channel();
    let host_release = Arc::new(tokio::sync::Notify::new());
    let host = Arc::new(runtime::ReusableHost::new(HostCapabilities::new(
        directories,
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(Stream {
            lease: Some(host_lease),
            entered: close_entered,
            release: Arc::clone(&host_release),
            fail: fail_host,
        })))),
        Box::new(Files),
    )));
    owner.accept_host(Arc::clone(&host));
    let stopper: Arc<dyn StopProfileRuntimePort> = Arc::new(FailedProfileStopper {
        _lease: stopper_lease,
    });
    if !fail_host {
        assert!(owner.register_profile(Arc::clone(&stopper)));
    }
    let primary = EngineError::new(1101, EngineErrorCategory::Unavailable, true);
    let (progress_input, progress) = StartupProgress::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let mut work =
        Some(runtime.spawn(async move { backup.retire_upgrade_backup_security_records().await }));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("backup entered");
    let backup_held = !available(&backup_path);
    let first_backup_error = if settle_first {
        release.send(()).expect("release backup");
        Some(
            runtime
                .block_on(work.take().expect("backup task"))
                .expect("worker")
                .is_err(),
        )
    } else {
        None
    };
    let rollback = runtime
        .block_on(stopper.stop_profile_runtime())
        .expect_err("stop failure");
    let source_preserved =
        uc_observability_contract::error_source::find_source::<std::io::Error>(&rollback)
            .is_some_and(|source| source.kind() == std::io::ErrorKind::PermissionDenied);
    if fail_host {
        owner.record_primary(primary.clone());
        owner.begin_cleanup();
        let closing_host = Arc::clone(&host);
        let closing = runtime.spawn(async move { closing_host.close_change_stream().await });
        close_wait
            .recv_timeout(Duration::from_secs(5))
            .expect("host closer entered");
        host_release.notify_one();
        let host_error = runtime
            .block_on(closing)
            .expect("close task")
            .expect_err("host close failure");
        owner.fail_host(primary.clone(), host_error);
    } else {
        owner.fail_rollback(primary.clone(), rollback);
    }
    owner.finish_construction_failure(
        EngineError::new(1108, EngineErrorCategory::Internal, false),
        progress_input,
        primary.clone(),
    );
    let pending = owner.wait_internal(Duration::ZERO);
    let stopper_held = !available(&stopper_path);
    let host_held = !available(&host_path);
    let host_not_called = !fail_host && close_wait.try_recv().is_err();
    let retry_while_pending = progress.snapshot().allowed_actions.retry;
    let backup_error = if let Some(result) = first_backup_error {
        result
    } else {
        release.send(()).expect("release backup");
        runtime
            .block_on(work.take().expect("backup task"))
            .expect("worker")
            .is_err()
    };
    let outcome = owner.wait_internal(Duration::from_secs(5));
    let repeated = owner.wait_internal(Duration::ZERO);
    let retained_error = matches!(&outcome, StartupOutcomeSnapshot::Failed { startup_error: Some(error), operation_error: Some(_), explicit_retry_supported: false, .. } if *error == primary);
    let retry_after = progress.snapshot().allowed_actions.retry;
    drop(stopper);
    drop(host);
    drop(owner);
    drop(progress);
    let stopper_retained = !available(&stopper_path);
    let host_retained = !available(&host_path);
    let expected_initial = if settle_first {
        matches!(pending, StartupOutcomeSnapshot::Failed { .. })
    } else {
        matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
    };
    let filename = if fail_host {
        "startup-host-failure-backup.json"
    } else if settle_first {
        "startup-rollback-backup-settled.json"
    } else if panic_after_release {
        "startup-rollback-backup-panic.json"
    } else {
        "startup-rollback-backup.json"
    };
    let artifact = serde_json::json!({"settle_first":settle_first, "panic_after_release":panic_after_release, "host_failure":fail_host, "expected_initial":expected_initial, "scope":"production startup owner and backup worker, controlled profile stopper, independent locks", "rollback_input_source_present":source_preserved, "retained_rollback_source":"not independently inspected", "pending_while_backup":format!("{pending:?}"), "backup_held":backup_held, "stopper_held":stopper_held, "host_held":host_held, "host_closer_not_called":host_not_called, "retry_while_pending":retry_while_pending, "backup_failed":backup_error, "result":format!("{outcome:?}"), "retained_errors":retained_error, "repeated_equal":outcome==repeated, "retry_after":retry_after, "stopper_retained_after_observer_drop":stopper_retained, "host_retained_after_observer_drop":host_retained, "full_startup_release":"not established", "device_acceptance":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join(filename),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        expected_initial
            && source_preserved
            && backup_held
            && stopper_held
            && host_held
            && (host_not_called || fail_host)
            && !retry_while_pending
            && backup_error
            && retained_error
            && outcome == repeated
            && !retry_after
            && (stopper_retained || fail_host)
            && host_retained
    );
    artifact
}

fn concurrent_host_and_backup(
    root: &Path,
    backup_first: bool,
    host_fails: bool,
) -> serde_json::Value {
    use uc_application::deps::RetireUpgradeBackupSecurityRecordsPort;
    use uc_infra_profile::security::ProfileUpgradeBackupStore;
    std::fs::create_dir_all(root).expect("root");
    let backup_path = root.join("backup.lease");
    let backup_lease = File::create(&backup_path).expect("backup lease");
    backup_lease.lock().expect("backup lock");
    let host_path = root.join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let (backup_entered, backup_wait) = mpsc::channel();
    let (backup_release, backup_release_wait) = mpsc::channel();
    let directories = HostDirectories::new(
        root.join("private"),
        root.join("cache"),
        root.join("temp"),
        root.join("logs"),
    );
    let storage = Arc::new(CoreGateStorage {
        panic_after_release: false,
        lease: Mutex::new(Some(backup_lease)),
        entered: backup_entered,
        release: Mutex::new(backup_release_wait),
    });
    let backup = ProfileUpgradeBackupStore::new(
        assembly::host::derive_app_paths(&directories),
        "controlled".into(),
        storage,
        root.join("backups"),
    );
    let owner = Arc::new(engine::startup_owner::StartupCleanupOwner::default());
    owner.register_backup_activity(backup.activity());
    let (host_entered, host_wait) = mpsc::channel();
    let host_release = Arc::new(tokio::sync::Notify::new());
    let host = Arc::new(runtime::ReusableHost::new(HostCapabilities::new(
        directories,
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(Stream {
            lease: Some(host_lease),
            entered: host_entered,
            release: Arc::clone(&host_release),
            fail: host_fails,
        })))),
        Box::new(Files),
    )));
    owner.accept_host(Arc::clone(&host));
    let primary = EngineError::new(1101, EngineErrorCategory::Unavailable, true);
    owner.record_primary(primary.clone());
    owner.begin_cleanup();
    let (progress_input, progress) = StartupProgress::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let mut work =
        Some(runtime.spawn(async move { backup.retire_upgrade_backup_security_records().await }));
    let closing_host = Arc::clone(&host);
    let close = runtime.spawn(async move { closing_host.close_change_stream().await });
    backup_wait
        .recv_timeout(Duration::from_secs(5))
        .expect("backup entered");
    host_wait
        .recv_timeout(Duration::from_secs(5))
        .expect("host close entered");
    owner.finish_construction_failure(
        EngineError::new(1108, EngineErrorCategory::Internal, false),
        progress_input,
        primary.clone(),
    );
    let initial = owner.wait_internal(Duration::ZERO);
    let both_held = !available(&backup_path) && !available(&host_path);
    let mut backup_error = None;
    if backup_first {
        backup_release.send(()).expect("release backup");
        backup_error = Some(
            runtime
                .block_on(work.take().expect("worker"))
                .expect("worker result")
                .is_err(),
        );
    } else {
        host_release.notify_one();
    }
    let mut close = Some(close);
    if !backup_first {
        let result = runtime
            .block_on(close.take().expect("close task"))
            .expect("close join");
        if let Err(error) = result {
            owner.fail_host(primary.clone(), error);
        } else {
            owner.host_closed();
        }
    }
    let between = owner.wait_internal(Duration::ZERO);
    if backup_first {
        host_release.notify_one();
        let result = runtime
            .block_on(close.take().expect("close task"))
            .expect("close join");
        if let Err(error) = result {
            owner.fail_host(primary.clone(), error);
        } else {
            owner.host_closed();
        }
    } else {
        backup_release.send(()).expect("release backup");
        backup_error = Some(
            runtime
                .block_on(work.take().expect("worker"))
                .expect("worker result")
                .is_err(),
        );
    }
    let result = owner.wait_internal(Duration::from_secs(5));
    let repeated = owner.wait_internal(Duration::ZERO);
    let valid_result = if host_fails {
        matches!(&result, StartupOutcomeSnapshot::Failed { startup_error: Some(error), operation_error: Some(_), explicit_retry_supported: false, .. } if *error == primary)
    } else {
        matches!(&result, StartupOutcomeSnapshot::Released { startup_error: Some(error), operation_error: Some(_) } if *error == primary)
    };
    let retry = progress.snapshot().allowed_actions.retry;
    drop(host);
    drop(owner);
    drop(progress);
    let host_held_after = !available(&host_path);
    let artifact = serde_json::json!({"scope":"production startup owner and backup worker, controlled host closer and independent locks", "backup_first":backup_first, "host_fails":host_fails, "initial":format!("{initial:?}"), "both_locks_held":both_held, "between":format!("{between:?}"), "backup_failed":backup_error, "result":format!("{result:?}"), "valid_result":valid_result, "repeated_equal":result==repeated, "retry_allowed":retry, "host_lock_held_after":host_held_after, "full_startup_release":"not established"});
    let filename = if backup_first && !host_fails {
        "startup-backup-first-host-success.json"
    } else if backup_first {
        "startup-backup-first-host-failure.json"
    } else if host_fails {
        "startup-host-failure-first-backup.json"
    } else {
        "startup-host-success-first-backup.json"
    };
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join(filename),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        matches!(initial, StartupOutcomeSnapshot::Cleaning { .. })
            && both_held
            && matches!(between, StartupOutcomeSnapshot::Cleaning { .. })
            && backup_error == Some(true)
            && valid_result
            && result == repeated
            && retry == !host_fails
            && host_held_after == host_fails
    );
    artifact
}

fn unpolled_failure_with_backup(root: &Path, kind: &str, settle_first: bool) -> serde_json::Value {
    use engine::startup_owner::UnpolledCleanupError;
    use uc_application::deps::RetireUpgradeBackupSecurityRecordsPort;
    use uc_infra_profile::security::ProfileUpgradeBackupStore;
    std::fs::create_dir_all(root).expect("root");
    let backup_path = root.join("backup.lease");
    let backup_lease = File::create(&backup_path).expect("backup lease");
    backup_lease.lock().expect("backup lock");
    let host_path = root.join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let (backup_entered, backup_wait) = mpsc::channel();
    let (backup_release, backup_release_wait) = mpsc::channel();
    let dirs = HostDirectories::new(
        root.join("private"),
        root.join("cache"),
        root.join("temp"),
        root.join("logs"),
    );
    let storage = Arc::new(CoreGateStorage {
        panic_after_release: false,
        lease: Mutex::new(Some(backup_lease)),
        entered: backup_entered,
        release: Mutex::new(backup_release_wait),
    });
    let backup = ProfileUpgradeBackupStore::new(
        assembly::host::derive_app_paths(&dirs),
        "controlled".into(),
        storage,
        root.join("backups"),
    );
    let owner = Arc::new(engine::startup_owner::StartupCleanupOwner::default());
    owner.register_backup_activity(backup.activity());
    let (host_entered, host_wait) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = Arc::new(runtime::ReusableHost::new(HostCapabilities::new(
        dirs,
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: kind == "panic",
            lease: Some(host_lease),
            entered: host_entered,
            release: Arc::new(tokio::sync::Notify::new()),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    )));
    owner.accept_host(Arc::clone(&host));
    let primary = EngineError::new(1101, EngineErrorCategory::Unavailable, true);
    owner.record_primary(primary.clone());
    owner.begin_cleanup();
    let (progress_input, progress) = StartupProgress::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let mut work =
        Some(runtime.spawn(async move { backup.retire_upgrade_backup_security_records().await }));
    backup_wait
        .recv_timeout(Duration::from_secs(5))
        .expect("backup entered");
    owner.finish_construction_failure(
        EngineError::new(1108, EngineErrorCategory::Internal, false),
        progress_input,
        primary.clone(),
    );
    let initial = owner.wait_internal(Duration::ZERO);
    let backup_held = !available(&backup_path);
    let host_held = !available(&host_path);
    let mut backup_error = None;
    if settle_first {
        backup_release.send(()).expect("release backup");
        backup_error = Some(
            runtime
                .block_on(work.take().expect("backup task"))
                .expect("worker result")
                .is_err(),
        );
    }
    let (error, callback_panicked) = match kind {
        "thread" => (
            UnpolledCleanupError::Thread(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "controlled worker creation failure",
            )),
            false,
        ),
        "executor" => (
            UnpolledCleanupError::Executor(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "controlled executor creation failure",
            )),
            false,
        ),
        "panic" => {
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(host.close_change_stream())
            }))
            .is_err();
            assert!(host_wait.recv_timeout(Duration::from_secs(2)).is_ok());
            (UnpolledCleanupError::CallbackPanicked, panicked)
        }
        _ => panic!("unknown controlled worker failure"),
    };
    let source_present = kind == "panic"
        || uc_observability_contract::error_source::find_source::<std::io::Error>(&error)
            .is_some_and(|source| source.kind() == std::io::ErrorKind::PermissionDenied);
    owner.fail_unpolled_worker(error);
    let between = owner.wait_internal(Duration::ZERO);
    let held_during = !available(&host_path);
    if !settle_first {
        backup_release.send(()).expect("release backup");
        backup_error = Some(
            runtime
                .block_on(work.take().expect("backup task"))
                .expect("worker result")
                .is_err(),
        );
    }
    let result = owner.wait_internal(Duration::from_secs(5));
    let repeated = owner.wait_internal(Duration::ZERO);
    let failed = matches!(&result, StartupOutcomeSnapshot::Failed { startup_error: Some(error), operation_error: Some(_), explicit_retry_supported: false, .. } if *error == primary);
    let retry = progress.snapshot().allowed_actions.retry;
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    drop(host);
    drop(owner);
    drop(progress);
    let host_retained = !available(&host_path);
    let expected_between = if settle_first {
        matches!(between, StartupOutcomeSnapshot::Failed { .. })
    } else {
        matches!(between, StartupOutcomeSnapshot::Cleaning { .. })
    };
    let filename = if settle_first {
        "startup-worker-backup-settled.json"
    } else {
        match kind {
            "thread" => "startup-worker-thread-backup.json",
            "executor" => "startup-worker-executor-backup.json",
            _ => "startup-worker-panic-backup.json",
        }
    };
    let artifact = serde_json::json!({"scope":"production startup owner and backup worker, controlled cleanup worker failure and independent locks", "failure_kind":kind, "backup_settled_first":settle_first, "worker_error_input_source_present":source_present, "callback_panicked":callback_panicked, "initial":format!("{initial:?}"), "between":format!("{between:?}"), "expected_between":expected_between, "backup_held":backup_held, "host_held":host_held, "host_held_during":held_during, "backup_failed":backup_error, "result":format!("{result:?}"), "terminal_failed":failed, "repeat_equal":result==repeated, "retry_allowed":retry, "host_close_calls":count, "host_retained":host_retained, "retained_error_source":"not independently inspected", "full_startup_release":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join(filename),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        matches!(initial, StartupOutcomeSnapshot::Cleaning { .. })
            && expected_between
            && source_present
            && backup_held
            && host_held
            && held_during
            && backup_error == Some(true)
            && failed
            && result == repeated
            && !retry
            && host_retained
            && count == usize::from(kind == "panic")
            && (kind != "panic" || callback_panicked)
    );
    artifact
}

#[cfg(feature = "dev-tools")]
struct DetachedCloseStream {
    host_lease: Option<File>,
    worker_lease: Option<File>,
    entered: mpsc::Sender<()>,
    release: Option<mpsc::Receiver<()>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    acknowledged: Arc<std::sync::atomic::AtomicBool>,
    ending: &'static str,
}

#[cfg(feature = "dev-tools")]
#[async_trait]
impl HostClipboardChangeStream for DetachedCloseStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let lease = self.worker_lease.take().expect("worker lease");
        let release = self.release.take().expect("worker gate");
        let entered = self.entered.clone();
        let worker = tokio::task::spawn_blocking(move || {
            entered.send(()).expect("worker entered");
            release.recv().expect("worker released");
            drop(lease);
        });
        match self.ending {
            "panic" => panic!("controlled close panic with active worker"),
            "error" => return Err(unavailable()),
            _ => {}
        }
        worker.await.expect("blocking worker joined");
        self.host_lease.take();
        self.acknowledged
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(feature = "dev-tools")]
struct DetachedCloseClipboard(Mutex<Option<DetachedCloseStream>>);

#[cfg(feature = "dev-tools")]
impl HostClipboard for DetachedCloseClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .lock()
            .expect("clipboard")
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

#[cfg(feature = "dev-tools")]
fn startup_detached_host_close(root: &Path, ending: &'static str) {
    use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
    std::fs::create_dir_all(root).expect("root");
    let ipv6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).expect("reserve port");
    let port = ipv6.local_addr().expect("port").port();
    let ipv4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok();
    let host_path = root.join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_path = root.join("worker.lease");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let acknowledged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (entered, waiting) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(DetachedCloseClipboard(Mutex::new(Some(
            DetachedCloseStream {
                host_lease: Some(host_lease),
                worker_lease: Some(worker_lease),
                entered,
                release: Some(blocked),
                calls: Arc::clone(&calls),
                acknowledged: Arc::clone(&acknowledged),
                ending,
            },
        )))),
        Box::new(Files),
    );
    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host,
        progress_input,
        lifecycle_input,
    ));
    waiting
        .recv_timeout(Duration::from_secs(20))
        .expect("host worker started");
    let host_held = !available(&host_path);
    let worker_held = !available(&worker_path);
    runtime.shutdown_timeout(Duration::from_millis(100));
    let initial = lifecycle.wait_release_internal(Duration::from_millis(300));
    let repeated_pending = lifecycle.wait_release_internal(Duration::ZERO);
    let held_after_executor = !available(&host_path) && !available(&worker_path);
    release.send(()).expect("release worker");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !available(&worker_path) {
        assert!(std::time::Instant::now() < deadline, "worker must finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(10));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    let host_available = available(&host_path);
    let terminal_valid = match &terminal {
        StartupOutcomeSnapshot::Failed {
            startup_error: Some(error),
            ..
        } => error.code() == 1101 && (ending == "success" || !host_available),
        StartupOutcomeSnapshot::Released {
            startup_error: Some(error),
            ..
        } => ending == "success" && error.code() == 1101 && host_available,
        _ => false,
    };
    let calls_final = calls.load(std::sync::atomic::Ordering::SeqCst);
    let host_acknowledged = acknowledged.load(std::sync::atomic::Ordering::SeqCst);
    let retry_allowed = progress.snapshot().allowed_actions.retry;
    drop(progress);
    drop(lifecycle);
    let filename = match ending {
        "panic" => "startup-detached-host-close-panic.json",
        "error" => "startup-detached-host-close-error.json",
        _ => "startup-detached-host-close.json",
    };
    let artifact = serde_json::json!({"callback_ending":ending, "scope":"real Engine failed network startup, detached host close worker, independent locks", "host_lock_held":host_held, "worker_lock_held":worker_held, "locks_held_after_executor_loss":held_after_executor, "initial":format!("{initial:?}"), "repeated_while_worker_active":format!("{repeated_pending:?}"), "worker_lock_available_after_completion":available(&worker_path), "terminal":format!("{terminal:?}"), "terminal_valid":terminal_valid, "repeat_equal":terminal == repeated, "host_close_calls":calls_final, "host_close_acknowledged":host_acknowledged, "host_lock_available_after_completion":host_available, "retry_allowed":retry_allowed, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join(filename),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    drop(ipv4);
    drop(ipv6);
    assert!(
        host_held
            && worker_held
            && held_after_executor
            && matches!(initial, StartupOutcomeSnapshot::Cleaning { .. })
            && matches!(repeated_pending, StartupOutcomeSnapshot::Cleaning { .. })
            && terminal_valid
            && terminal == repeated
            && calls_final == 1
            && host_acknowledged == (ending == "success")
            && host_available == (ending == "success")
    );
}

#[cfg(feature = "dev-tools")]
struct RetryWorkerStream {
    host_lease: Option<File>,
    worker_lease: Option<File>,
    started: mpsc::Sender<()>,
    gate: Option<mpsc::Receiver<()>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    ending: &'static str,
}

#[cfg(feature = "dev-tools")]
#[async_trait]
impl HostClipboardChangeStream for RetryWorkerStream {
    fn supports_shutdown_retry(&self) -> bool {
        true
    }
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            return Err(unavailable());
        }
        let lease = self.worker_lease.take().expect("worker lease");
        let gate = self.gate.take().expect("worker gate");
        let started = self.started.clone();
        let (running, ready) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            started.send(()).expect("worker entered");
            running.send(()).expect("worker ready");
            gate.recv().expect("worker released");
            drop(lease);
        });
        ready.await.expect("worker running");
        match self.ending {
            "error" | "drop-error" => Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::PermissionDenied,
                "controlled retry failure",
            )
            .with_source(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "controlled retry source",
            )))),
            "panic" | "drop-callback" => panic!("controlled retry panic"),
            _ => {
                self.host_lease.take();
                Ok(())
            }
        }
    }
}

#[cfg(feature = "dev-tools")]
struct RetryWorkerClipboard(Mutex<Option<RetryWorkerStream>>);
#[cfg(feature = "dev-tools")]
impl HostClipboard for RetryWorkerClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .lock()
            .expect("clipboard")
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

#[cfg(feature = "dev-tools")]
fn host_retry_blocking_worker(root: &Path, ending: &'static str) {
    std::fs::create_dir_all(root).expect("root");
    let host_path = root.join("host.lease");
    let worker_path = root.join("worker.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    host_lease.lock().expect("host lock");
    worker_lease.lock().expect("worker lock");
    let (started, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryWorkerClipboard(Mutex::new(Some(RetryWorkerStream {
            host_lease: Some(host_lease),
            worker_lease: Some(worker_lease),
            started,
            gate: Some(gate),
            calls: Arc::clone(&calls),
            ending,
        })))),
        Box::new(Files),
    );
    let socket = std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).expect("port");
    let port = socket.local_addr().expect("address").port();
    let ipv4 = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).ok();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (progress, _) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    let startup = runtime.block_on(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host,
        progress,
        input,
    ));
    let primary = match startup {
        Err(error) => error,
        Ok(_) => panic!("occupied port accepted"),
    };
    let initial = control.wait_release_internal(Duration::from_secs(5));
    let retryable = matches!(
        initial,
        StartupOutcomeSnapshot::Failed {
            explicit_retry_supported: true,
            ..
        }
    );
    if ending == "init-panic" {
        engine::startup_owner::PROBE_HOST_RETRY_EXECUTOR_PANIC
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let accepted = control.retry_host_internal(Duration::ZERO);
        let result = control.wait_release_internal(Duration::from_secs(2));
        let repeated = control.wait_release_internal(Duration::ZERO);
        let host_held = !available(&host_path);
        let worker_held = !available(&worker_path);
        let count = calls.load(std::sync::atomic::Ordering::SeqCst);
        let panic_retained = control.probe_retry_executor_panic();
        let failed = matches!((&initial, &result), (StartupOutcomeSnapshot::Failed { cleanup_error: previous, .. }, StartupOutcomeSnapshot::Failed { startup_error: Some(error), operation_error: Some(operation), explicit_retry_supported: false, .. }) if *error == primary && operation == previous);
        let retry = control.retry_host_internal(Duration::ZERO);
        drop(control);
        let retained = !available(&host_path) && !available(&worker_path);
        let artifact = serde_json::json!({"scope":"real Engine startup failure with injected retry executor construction panic", "primary":format!("{primary:?}"), "initial":format!("{initial:?}"), "accepted":format!("{accepted:?}"), "result":format!("{result:?}"), "stable":result==repeated && result==retry, "panic_retained":panic_retained, "host_lock_held":host_held, "worker_lock_held":worker_held, "locks_retained_after_observer_loss":retained, "close_calls":count, "native_mobile":"not established"});
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("directory");
            std::fs::write(
                Path::new(&directory).join("startup-retry-executor-panic.json"),
                serde_json::to_vec_pretty(&artifact).expect("json"),
            )
            .expect("artifact");
        }
        assert!(
            retryable
                && failed
                && panic_retained
                && result == repeated
                && result == retry
                && host_held
                && worker_held
                && retained
                && count == 1
        );
        return;
    }
    if matches!(ending, "drop-panic" | "drop-error" | "drop-callback") {
        engine::startup_owner::PROBE_HOST_RETRY_DROP_PANIC
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let retry = control.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("retry worker entered");
    let pending = control.wait_release_internal(Duration::from_millis(300));
    let joined = control.retry_host_internal(Duration::ZERO);
    let worker_held = !available(&worker_path);
    let host_held = !available(&host_path);
    let count_pending = calls.load(std::sync::atomic::Ordering::SeqCst);
    release.send(()).expect("release worker");
    let final_result = control.wait_release_internal(Duration::from_secs(
        if matches!(ending, "drop-panic" | "drop-error" | "drop-callback") {
            2
        } else {
            10
        },
    ));
    let repeated = control.wait_release_internal(Duration::ZERO);
    let worker_free = available(&worker_path);
    let host_free = available(&host_path);
    let count_final = calls.load(std::sync::atomic::Ordering::SeqCst);
    let drop_panic_retained = if ending == "drop-callback" {
        control.probe_retry_callback_and_drop_panic()
    } else {
        !matches!(ending, "drop-panic" | "drop-error") || control.probe_retry_executor_drop_panic()
    };
    let terminal_retry_rejected = !matches!(ending, "drop-panic" | "drop-error" | "drop-callback")
        || control.retry_host_internal(Duration::ZERO) == final_result;
    let errors_preserved = !matches!(ending, "error" | "drop-error")
        || control.probe_host_cleanup_errors()
            == vec![
                (HostCapabilityErrorCategory::Unavailable, None),
                (
                    HostCapabilityErrorCategory::PermissionDenied,
                    Some(std::io::ErrorKind::PermissionDenied),
                ),
            ];
    let terminal_valid = match &final_result {
        StartupOutcomeSnapshot::Released {
            startup_error: Some(error),
            operation_error: Some(_),
        } => ending == "success" && *error == primary && host_free,
        StartupOutcomeSnapshot::Failed {
            startup_error: Some(error),
            operation_error: Some(operation_error),
            explicit_retry_supported,
            ..
        } => {
            ending != "success"
                && *error == primary
                && matches!(&initial, StartupOutcomeSnapshot::Failed { cleanup_error, .. } if cleanup_error == operation_error)
                && (host_free == (ending == "drop-panic"))
                && *explicit_retry_supported == (ending == "error")
        }
        _ => false,
    };
    let artifact = serde_json::json!({"scope":"real Engine port-conflict startup and explicitly retried host close", "ending":ending, "primary":format!("{primary:?}"), "initial":format!("{initial:?}"), "retry":format!("{retry:?}"), "pending":format!("{pending:?}"), "joined":format!("{joined:?}"), "worker_held":worker_held, "host_held":host_held, "count_pending":count_pending, "worker_free":worker_free, "host_free":host_free, "final":format!("{final_result:?}"), "drop_panic_retained":drop_panic_retained, "host_error_sources_preserved":errors_preserved, "terminal_retry_rejected":terminal_retry_rejected, "terminal_valid":terminal_valid, "repeat_equal":final_result == repeated, "count_final":count_final, "device_acceptance":"not established"});
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            Path::new(&directory).join(format!("startup-retry-worker-{ending}.json")),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    drop(ipv4);
    drop(socket);
    assert!(
        retryable
            && matches!(retry, StartupOutcomeSnapshot::Cleaning { .. })
            && matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
            && pending == joined
            && worker_held
            && host_held == (ending != "success" && ending != "drop-panic")
            && count_pending == 2
            && worker_free
            && terminal_valid
            && drop_panic_retained
            && errors_preserved
            && terminal_retry_rejected
            && final_result == repeated
            && count_final == 2
    );
}

#[cfg(feature = "dev-tools")]
fn real_startup_rollback_task_failure(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(Stream {
            lease: Some(lease),
            entered,
            release: Arc::new(tokio::sync::Notify::new()),
            fail: false,
        })))),
        Box::new(Files),
    );
    let socket = std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).expect("port");
    let port = socket.local_addr().expect("address").port();
    let ipv4 = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).ok();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (progress, observer) = StartupProgress::channel();
    let (input, control) = StartupLifecycle::channel();
    runtime::PROBE_STARTUP_TASK_PANIC.store(true, std::sync::atomic::Ordering::Release);
    let result = runtime.block_on(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host,
        progress,
        input,
    ));
    let primary = match result {
        Err(error) => error,
        Ok(_) => panic!("occupied port accepted"),
    };
    let failed = control.wait_release_internal(Duration::from_secs(10));
    let repeated = control.wait_release_internal(Duration::ZERO);
    let retry = control.retry_host_internal(Duration::ZERO);
    let retained_source = control.probe_rollback_task_panic();
    let host_held = !available(&path);
    let host_close_not_called = waiting.try_recv().is_err();
    let startup_retry_blocked = !observer.snapshot().allowed_actions.retry;
    let valid = matches!(failed, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), explicit_retry_supported: false, .. } if *error == primary && error.code() == 1101)
        && failed == repeated
        && failed == retry
        && retained_source
        && host_held
        && host_close_not_called
        && startup_retry_blocked;
    drop(observer);
    drop(control);
    drop(runtime);
    let retained_after_observer_drop = !available(&path);
    let artifact = serde_json::json!({"scope":"actual Engine port-conflict startup, production task registry and rollback, controlled task panic", "primary":format!("{primary:?}"), "failed":format!("{failed:?}"), "repeat_equal":failed == repeated, "retry_joined":failed == retry, "task_panic_source_retained":retained_source, "host_held":host_held, "host_close_not_called":host_close_not_called, "startup_retry_blocked":startup_retry_blocked, "host_retained_after_observer_drop":retained_after_observer_drop, "network_closer_failure":"not tested", "valid":valid && retained_after_observer_drop});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("startup-real-rollback-task-panic.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    drop(ipv4);
    drop(socket);
    assert!(valid && retained_after_observer_drop);
}

#[cfg(feature = "dev-tools")]
struct HandoffStorage {
    values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}
#[cfg(feature = "dev-tools")]
impl HostSecureStorage for HandoffStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        if key.starts_with("kek:v1:") {
            if let Some((entered, gate)) = self.gate.lock().expect("gate").take() {
                entered.send(()).expect("storage entered");
                gate.recv_timeout(Duration::from_secs(20))
                    .expect("storage released");
            }
        }
        Ok(self.values.lock().expect("values").get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.values
            .lock()
            .expect("values")
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.values.lock().expect("values").remove(key);
        Ok(())
    }
}

#[cfg(feature = "dev-tools")]
struct HandoffWorkerStream {
    host_lease: Option<File>,
    worker_lease: Option<File>,
    entered: mpsc::Sender<()>,
    gate: Option<mpsc::Receiver<()>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    ending: &'static str,
}
#[cfg(feature = "dev-tools")]
#[async_trait]
impl HostClipboardChangeStream for HandoffWorkerStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let lease = self.worker_lease.take().expect("worker lease");
        let gate = self.gate.take().expect("worker gate");
        let entered = self.entered.clone();
        let (ready, started) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            entered.send(()).expect("worker entered");
            ready.send(()).expect("worker ready");
            gate.recv().expect("worker released");
            drop(lease);
        });
        started.await.expect("worker started");
        match self.ending {
            "error" => return Err(unavailable()),
            "panic" => panic!("controlled undelivered close panic"),
            _ => {}
        }
        worker.await.expect("worker joined");
        self.host_lease.take();
        Ok(())
    }
}
#[cfg(feature = "dev-tools")]
struct HandoffClipboard(Mutex<Option<HandoffWorkerStream>>);
#[cfg(feature = "dev-tools")]
impl HostClipboard for HandoffClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .lock()
            .expect("clipboard")
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

#[cfg(feature = "dev-tools")]
fn undelivered_engine_executor_loss(root: &Path, ending: &'static str) {
    std::fs::create_dir_all(root).expect("root");
    let values = Arc::new(Mutex::new(HashMap::new()));
    let dirs = || {
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        )
    };
    let initial_host = HostCapabilities::new(
        dirs(),
        Box::new(HandoffStorage {
            values: Arc::clone(&values),
            gate: Mutex::new(None),
        }),
        Box::new(Clipboard(Mutex::new(None))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), initial_host)
            .await
            .expect("initial engine");
        engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("fixture".into()),
                passphrase: SecretString::new("fixture passphrase"),
                passphrase_confirmation: SecretString::new("fixture passphrase"),
            }))
            .await
            .expect("create fixture");
        engine
            .shutdown(Duration::from_secs(15))
            .await
            .expect("shutdown fixture");
    });
    let host_path = root.join("host.lease");
    let worker_path = root.join("worker.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let (storage_entered, storage_wait) = mpsc::channel();
    let (storage_release, storage_gate) = mpsc::channel();
    let (worker_entered, worker_wait) = mpsc::channel();
    let (worker_release, worker_gate) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        dirs(),
        Box::new(HandoffStorage {
            values,
            gate: Mutex::new(Some((storage_entered, storage_gate))),
        }),
        Box::new(HandoffClipboard(Mutex::new(Some(HandoffWorkerStream {
            host_lease: Some(host_lease),
            worker_lease: Some(worker_lease),
            entered: worker_entered,
            gate: Some(worker_gate),
            calls: Arc::clone(&calls),
            ending,
        })))),
        Box::new(Files),
    );
    let (progress, observer) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    storage_wait
        .recv_timeout(Duration::from_secs(20))
        .expect("storage gate");
    caller.abort();
    let injection = match ending {
        "thread" => 1,
        "executor" => 2,
        _ => 0,
    };
    engine::startup_owner::PROBE_ENGINE_CLEANUP_FAILURE
        .store(injection, std::sync::atomic::Ordering::Release);
    storage_release.send(()).expect("release storage");
    if injection != 0 {
        let failed = lifecycle.wait_release_internal(Duration::from_secs(20));
        let repeated = lifecycle.wait_release_internal(Duration::ZERO);
        let retry = lifecycle.retry_host_internal(Duration::ZERO);
        let source = lifecycle.probe_cleanup_worker_failure();
        let source_retained = source == Some((ending, std::io::ErrorKind::PermissionDenied));
        let locks_held = !available(&host_path) && !available(&worker_path);
        let no_close =
            calls.load(std::sync::atomic::Ordering::SeqCst) == 0 && worker_wait.try_recv().is_err();
        let retry_blocked = !observer.snapshot().allowed_actions.retry;
        runtime.shutdown_timeout(Duration::from_millis(100));
        drop(observer);
        drop(lifecycle);
        let locks_retained = !available(&host_path) && !available(&worker_path);
        let valid = matches!(
            failed,
            StartupOutcomeSnapshot::Failed {
                explicit_retry_supported: false,
                ..
            }
        ) && failed == repeated
            && failed == retry
            && source_retained
            && locks_held
            && locks_retained
            && no_close
            && retry_blocked;
        let artifact = serde_json::json!({"scope":"undelivered real Engine with controlled cleanup worker creation failure", "injected":ending, "failed":format!("{failed:?}"), "stable":failed == repeated && failed == retry, "source_retained_at_result":source_retained, "locks_held":locks_held, "locks_retained_after_observer_loss":locks_retained, "no_host_close":no_close, "retry_blocked":retry_blocked, "valid":valid, "actual_operating_system_failure":"not established"});
        if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&dir).expect("directory");
            std::fs::write(
                Path::new(&dir).join(format!("startup-undelivered-worker-{ending}.json")),
                serde_json::to_vec_pretty(&artifact).expect("json"),
            )
            .expect("artifact");
        }
        assert!(valid);
        return;
    }
    worker_wait
        .recv_timeout(Duration::from_secs(20))
        .expect("host worker started");
    let before = lifecycle.wait_release_internal(Duration::ZERO);
    let both_held_before = !available(&host_path) && !available(&worker_path);
    runtime.shutdown_timeout(Duration::from_millis(100));
    let after = lifecycle.wait_release_internal(Duration::from_millis(300));
    let both_held_after = !available(&host_path) && !available(&worker_path);
    worker_release.send(()).expect("release worker");
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !available(&worker_path) {
        assert!(std::time::Instant::now() < until, "worker must finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(10));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    let host_free = available(&host_path);
    let call_count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let pending = matches!(after, StartupOutcomeSnapshot::Cleaning { .. });
    let valid = ending == "success"
        && matches!(terminal, StartupOutcomeSnapshot::Released { .. })
        && host_free
        || ending != "success"
            && matches!(terminal, StartupOutcomeSnapshot::Failed { .. })
            && !host_free;
    let artifact = serde_json::json!({"scope":"undelivered real Engine, executor loss during host close, independent host and blocking-worker locks", "ending":ending, "before":format!("{before:?}"), "after_executor_loss":format!("{after:?}"), "pending_while_worker_held":pending, "both_held_before":both_held_before, "both_held_after":both_held_after, "terminal":format!("{terminal:?}"), "repeat_equal":terminal==repeated, "host_free":host_free, "close_calls":call_count, "startup_retry_allowed":observer.snapshot().allowed_actions.retry, "valid":valid, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join(format!("startup-undelivered-executor-worker-{ending}.json")),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        both_held_before
            && both_held_after
            && pending
            && valid
            && terminal == repeated
            && call_count == 1
    );
}

#[cfg(feature = "dev-tools")]
fn failed_log_begin_after_runtime(root: &Path, ending: &'static str) {
    std::fs::create_dir_all(root).expect("root");
    let host_path = root.join("host.lease");
    let worker_path = root.join("worker.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(HandoffClipboard(Mutex::new(Some(HandoffWorkerStream {
            host_lease: Some(host_lease),
            worker_lease: Some(worker_lease),
            entered,
            gate: Some(gate),
            calls: Arc::clone(&calls),
            ending,
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (progress, observer) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    engine::probe_fail_log_begin();
    let result = runtime.block_on(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    let primary = match result {
        Err(error) => error,
        Ok(_) => panic!("log failure was not injected"),
    };
    let worker_started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    let pending = lifecycle.wait_release_internal(Duration::from_millis(300));
    let joined = lifecycle.retry_host_internal(Duration::ZERO);
    let host_held = !available(&host_path);
    let worker_held = !available(&worker_path);
    runtime.shutdown_timeout(Duration::from_millis(100));
    let after_executor = lifecycle.wait_release_internal(Duration::ZERO);
    let held_after_executor = !available(&host_path) && !available(&worker_path);
    if worker_started {
        release.send(()).expect("release worker");
    }
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(10));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    let host_free = available(&host_path);
    let worker_free = available(&worker_path);
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid_terminal = match &terminal {
        StartupOutcomeSnapshot::Released {
            startup_error: Some(error),
            ..
        } => ending == "success" && *error == primary && host_free,
        StartupOutcomeSnapshot::Failed {
            startup_error: Some(error),
            ..
        } => ending != "success" && *error == primary && !host_free,
        _ => false,
    };
    let valid = worker_started
        && matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
        && pending == joined
        && host_held
        && worker_held
        && held_after_executor
        && matches!(after_executor, StartupOutcomeSnapshot::Cleaning { .. })
        && worker_free
        && valid_terminal
        && terminal == repeated
        && count == 1;
    let artifact = serde_json::json!({"scope":"real Engine, injected post-construction log registration failure, independent host and worker locks", "ending":ending, "primary":format!("{primary:?}"), "worker_started":worker_started, "pending":format!("{pending:?}"), "joined":format!("{joined:?}"), "host_held":host_held, "worker_held":worker_held, "after_executor":format!("{after_executor:?}"), "held_after_executor":held_after_executor, "terminal":format!("{terminal:?}"), "repeat_equal":terminal==repeated, "host_free":host_free, "worker_free":worker_free, "close_calls":count, "startup_retry_allowed":observer.snapshot().allowed_actions.retry, "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join(format!("startup-log-begin-failure-{ending}.json")),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid);
}

#[cfg(feature = "dev-tools")]
fn independent_registration_cleanup(root: &Path) {
    use std::time::Instant;
    use uc_observability_runtime::{
        DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig,
        NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
        ProcessObservabilityRuntime, SignalResult,
    };
    let log_root = root.join("process-logs");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_network_logs(
        NetworkLogConfig::new(
            &log_root,
            NetworkProcessRole::MobileApp,
            NetworkLogBudget {
                record_bytes: 1024,
                queue_records: 32,
                file_bytes: 4096,
                run_bytes: 8192,
                root_bytes: 16384,
                retention: Duration::from_secs(60),
            },
        )
        .expect("network"),
    );
    let _observability = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut first, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        first.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let fifo = log_root.join("network/.cleanup.lock");
    std::fs::remove_file(&fifo).expect("remove lock");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo")
        .success());
    let (mut blocker, _blocked_resume) = EngineLogActivity::begin(Duration::from_millis(50));
    let _blocked_park = blocker.end(Duration::from_millis(50));
    let blocked_fifo = log_root.join("network/blocked.fifo");
    std::fs::rename(&fifo, &blocked_fifo).expect("rename fifo");
    std::fs::write(&fifo, []).expect("restore lock");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let host_path = root.join("cancelled-host.lease");
    let worker_path = root.join("cancelled-worker.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let (entered, waiting_worker) = mpsc::channel();
    let (release, worker_gate) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(HandoffClipboard(Mutex::new(Some(HandoffWorkerStream {
            host_lease: Some(host_lease),
            worker_lease: Some(worker_lease),
            entered,
            gate: Some(worker_gate),
            calls: Arc::clone(&calls),
            ending: "success",
        })))),
        Box::new(Files),
    );
    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress_input,
        lifecycle_input,
    ));
    let until = Instant::now() + Duration::from_secs(20);
    while progress.snapshot().state != StartupState::StartingServices {
        assert!(
            Instant::now() < until && !caller.is_finished(),
            "startup did not reach services"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(!caller.is_finished(), "registration must be blocked");
    runtime.shutdown_timeout(Duration::from_millis(50));
    let initial = lifecycle.wait_release_internal(Duration::ZERO);
    let held_before = !available(&host_path) && !available(&worker_path);
    let (opened, waiting_fifo) = mpsc::channel();
    let (close, closing) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let fifo = File::open(&blocked_fifo).expect("fifo");
        opened.send(()).expect("opened");
        closing.recv().expect("close fifo");
        drop(fifo);
    });
    waiting_fifo
        .recv_timeout(Duration::from_secs(5))
        .expect("fifo opened");
    let worker_started = waiting_worker.recv_timeout(Duration::from_secs(10)).is_ok();
    let during = lifecycle.wait_release_internal(Duration::from_millis(300));
    let held_during = !available(&host_path) && !available(&worker_path);
    if worker_started {
        release.send(()).expect("release worker");
    }
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(10));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    let host_free = available(&host_path);
    let worker_free = available(&worker_path);
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    close.send(()).expect("close fifo");
    reader.join().expect("reader");
    let valid = matches!(
        initial,
        StartupOutcomeSnapshot::Constructing { .. } | StartupOutcomeSnapshot::Cleaning { .. }
    ) && held_before
        && worker_started
        && matches!(during, StartupOutcomeSnapshot::Cleaning { .. })
        && held_during
        && matches!(terminal, StartupOutcomeSnapshot::Released { .. })
        && terminal == repeated
        && host_free
        && worker_free
        && count == 1;
    let artifact = serde_json::json!({"scope":"real Engine, delayed actual network registration and loss of constructor executor", "initial":format!("{initial:?}"), "worker_started":worker_started, "during":format!("{during:?}"), "terminal":format!("{terminal:?}"), "held_before":held_before, "held_during":held_during, "host_free":host_free, "worker_free":worker_free, "close_calls":count, "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("startup-independent-executor.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid, "startup work must outlive the constructor executor");
}

#[cfg(feature = "dev-tools")]
fn delivered_engine_executor_loss(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    let host_path = root.join("host.lease");
    let worker_path = root.join("worker.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(HandoffClipboard(Mutex::new(Some(HandoffWorkerStream {
            host_lease: Some(host_lease),
            worker_lease: Some(worker_lease),
            entered,
            gate: Some(gate),
            calls: Arc::clone(&calls),
            ending: "success",
        })))),
        Box::new(Files),
    );
    let parent = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("parent");
    let (progress, _) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    let (engine, events) = parent
        .block_on(Engine::start_with_lifecycle(
            EngineConfig::new("2.0.0"),
            host,
            progress,
            input,
        ))
        .expect("engine");
    let delivered = lifecycle.wait_release_internal(Duration::ZERO);
    parent.shutdown_timeout(Duration::from_millis(100));
    let engine = Arc::new(engine);
    let observer = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("observer");
    let closing = Arc::clone(&engine);
    let task = observer.spawn(async move { closing.shutdown_until_released().await });
    waiting
        .recv_timeout(Duration::from_secs(15))
        .expect("worker started");
    let held = !available(&host_path) && !available(&worker_path) && !task.is_finished();
    release.send(()).expect("release");
    let terminal = observer.block_on(task).expect("shutdown task");
    let repeated = observer.block_on(engine.shutdown_until_released());
    let executor_stopped = engine.probe_owned_executor_stopped();
    let host_free = available(&host_path);
    let worker_free = available(&worker_path);
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid = matches!(delivered, StartupOutcomeSnapshot::Delivered)
        && held
        && matches!(terminal, ResourceReleaseOutcome::Released { .. })
        && terminal == repeated
        && executor_stopped
        && host_free
        && worker_free
        && count == 1;
    let artifact = serde_json::json!({"scope":"delivered real Engine, lost constructor executor, shutdown from a new executor and independent locks", "delivered":format!("{delivered:?}"), "held_while_pending":held, "terminal":format!("{terminal:?}"), "repeated_equal":terminal==repeated, "owned_executor_stopped":executor_stopped, "host_free":host_free, "worker_free":worker_free, "close_calls":count, "valid":valid, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("delivered-engine-owned-executor.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    drop(events);
    assert!(valid);
}

#[cfg(feature = "dev-tools")]
fn terminated_active_lifecycle(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    let host_path = root.join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let close_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (closer_entered, _) = mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(host_lease),
            entered: closer_entered,
            release: Arc::new(tokio::sync::Notify::new()),
            calls: Arc::clone(&close_calls),
        })))),
        Box::new(Files),
    );
    let creator = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("creator");
    let (engine, events) = creator
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .expect("engine");
    let engine = Arc::new(engine);
    let path = root.join("worker.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    assert!(engine.probe_spawn_owned_blocking(move || {
        entered.send(()).expect("entered");
        gate.recv().expect("release");
        drop(lease);
    }));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("blocking worker entered");
    engine.probe_hold_lifecycle();
    let request_engine = Arc::clone(&engine);
    let request = creator.spawn(async move { request_engine.suspend().await });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !engine.probe_lifecycle_held() {
        assert!(
            std::time::Instant::now() < deadline,
            "lifecycle request not accepted"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    engine.probe_fail_owned_executor();
    let settling_engine = Arc::clone(&engine);
    let mut settling =
        creator.spawn(async move { settling_engine.settle_terminated_shutdown().await });
    std::thread::sleep(Duration::from_millis(200));
    let pending_while_held = !settling.is_finished() && !available(&path);
    release.send(()).expect("release worker");
    let outcome = creator
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), &mut settling).await });
    let failed = matches!(outcome, Ok(Ok(ResourceReleaseOutcome::Failed { .. })));
    let repeated_failed = matches!(
        creator.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), engine.settle_terminated_shutdown()).await
        }),
        Ok(ResourceReleaseOutcome::Failed { .. })
    );
    let request_failed = matches!(
        creator.block_on(async { tokio::time::timeout(Duration::from_secs(5), request).await }),
        Ok(Ok(Err(_)))
    );
    let free = available(&path);
    let host_held = !available(&host_path);
    let calls = close_calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid = pending_while_held
        && failed
        && repeated_failed
        && request_failed
        && free
        && host_held
        && calls == 0;
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(Path::new(&dir).join("terminated-active-lifecycle.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine, controlled active lifecycle queue and executor panic, independent host and blocking locks", "pending_while_held":pending_while_held, "failed_after_worker":failed, "repeated_failed":repeated_failed, "request_failed":request_failed, "worker_lock_released":free, "host_lock_retained":host_held, "host_close_calls":calls, "valid":valid, "native_mobile":"not established"})).expect("json")).expect("artifact");
    }
    drop(events);
    drop(creator);
    assert!(valid, "terminated lifecycle request did not settle");
}

#[cfg(feature = "dev-tools")]
fn terminated_shutdown_waiter(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(None))),
        Box::new(Files),
    );
    let creator = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("creator");
    let (engine, events) = creator
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .expect("engine");
    let engine = Arc::new(engine);
    let path = root.join("worker.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    assert!(engine.probe_spawn_owned_blocking(move || {
        entered.send(()).expect("started");
        gate.recv().expect("release");
        drop(lease);
    }));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("worker started");
    let caller = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("caller");
    let closing = Arc::clone(&engine);
    caller.spawn(async move { closing.shutdown_until_released().await });
    let until = std::time::Instant::now() + Duration::from_secs(15);
    while !engine.probe_owned_executor_stopping() {
        assert!(
            std::time::Instant::now() < until,
            "executor stop not accepted"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let held_before = !available(&path);
    caller.shutdown_timeout(Duration::from_millis(100));
    let observer = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("observer");
    let pending = observer.block_on(engine.shutdown_with_outcome(Duration::from_millis(300)));
    let held_after = !available(&path);
    release.send(()).expect("release");
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !available(&path) {
        assert!(std::time::Instant::now() < until, "worker did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    let final_result = observer.block_on(engine.shutdown_until_released());
    let repeated = observer.block_on(engine.shutdown_until_released());
    let stopped = engine.probe_owned_executor_stopped();
    let valid = held_before
        && held_after
        && matches!(pending, ResourceReleaseOutcome::Pending)
        && matches!(final_result, ResourceReleaseOutcome::Failed { .. })
        && final_result == repeated
        && stopped;
    let artifact = serde_json::json!({"scope":"real delivered Engine, accepted owned-executor blocking task and terminated shutdown caller", "held_before":held_before, "held_after":held_after, "pending":format!("{pending:?}"), "final":format!("{final_result:?}"), "repeat_equal":final_result==repeated, "owned_executor_stopped":stopped, "lock_free":available(&path), "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("terminated-owned-executor-waiter.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    drop(events);
    drop(creator);
    assert!(valid);
}

#[cfg(feature = "dev-tools")]
fn owned_start_failure(root: &Path, stage: &'static str) {
    std::fs::create_dir_all(root).expect("root");
    let host_path = root.join("host.lease");
    let worker_path = root.join("worker.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(HandoffClipboard(Mutex::new(Some(HandoffWorkerStream {
            host_lease: Some(host_lease),
            worker_lease: Some(worker_lease),
            entered,
            gate: Some(gate),
            calls: Arc::clone(&calls),
            ending: "success",
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (progress, observer) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    engine::probe_fail_owned_start(if stage == "thread" { 1 } else { 2 });
    let result = runtime.block_on(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    let primary = match result {
        Err(error) => error,
        Ok(_) => panic!("creation failure was not injected"),
    };
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("host worker");
    let pending = lifecycle.wait_release_internal(Duration::from_millis(200));
    let repeated_pending = lifecycle.retry_host_internal(Duration::ZERO);
    let held = !available(&host_path) && !available(&worker_path);
    let source = lifecycle.probe_owned_start_failure();
    release.send(()).expect("release worker");
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(10));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    let host_free = available(&host_path);
    let worker_free = available(&worker_path);
    let source_retained = lifecycle.probe_owned_start_failure()
        == Some((stage, std::io::ErrorKind::PermissionDenied));
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid = matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
        && pending == repeated_pending
        && held
        && source == Some((stage, std::io::ErrorKind::PermissionDenied))
        && source_retained
        && matches!(terminal, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), .. } if *error == primary)
        && terminal == repeated
        && host_free
        && worker_free
        && count == 1
        && observer.snapshot().allowed_actions.retry;
    let artifact = serde_json::json!({"scope":"real Engine input and host close, injected owned executor creation failure", "injected":stage, "primary":format!("{primary:?}"), "pending":format!("{pending:?}"), "held":held, "source_before":format!("{source:?}"), "source_retained":source_retained, "terminal":format!("{terminal:?}"), "repeat_equal":terminal==repeated, "host_free":host_free, "worker_free":worker_free, "close_calls":count, "valid":valid, "actual_operating_system_failure":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join(format!("owned-start-{stage}.json")),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid);
}

#[cfg(feature = "dev-tools")]
fn owned_start_failure_host_retry(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (progress, observer) = StartupProgress::channel();
    let (input, lifecycle) = StartupLifecycle::channel();
    engine::probe_fail_owned_start(1);
    let result = runtime.block_on(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host,
        progress,
        input,
    ));
    let primary = match result {
        Err(error) => error,
        Ok(_) => panic!("thread failure was not injected"),
    };
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("first close");
    let pending = lifecycle.wait_release_internal(Duration::ZERO);
    let held_first = !available(&path);
    release.notify_one();
    let failed = lifecycle.wait_release_internal(Duration::from_secs(5));
    let repeated = lifecycle.wait_release_internal(Duration::ZERO);
    let advertised = matches!(failed, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), explicit_retry_supported: true, .. } if *error == primary);
    let first_count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let pending_second = lifecycle.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("second close");
    let joined = lifecycle.retry_host_internal(Duration::ZERO);
    let held_second = !available(&path);
    release.notify_one();
    let second = lifecycle.wait_release_internal(Duration::from_secs(5));
    let second_retryable = matches!(
        second,
        StartupOutcomeSnapshot::Failed {
            explicit_retry_supported: true,
            ..
        }
    ) && !available(&path);
    let pending_third = lifecycle.retry_host_internal(Duration::ZERO);
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("third close");
    release.notify_one();
    let terminal = lifecycle.wait_release_internal(Duration::from_secs(5));
    let last = lifecycle.wait_release_internal(Duration::ZERO);
    let free = available(&path);
    let source = lifecycle.probe_owned_start_failure()
        == Some(("thread", std::io::ErrorKind::PermissionDenied));
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid = matches!(pending, StartupOutcomeSnapshot::Cleaning { .. })
        && held_first
        && advertised
        && failed == repeated
        && first_count == 1
        && matches!(pending_second, StartupOutcomeSnapshot::Cleaning { .. })
        && joined == pending_second
        && held_second
        && second_retryable
        && matches!(pending_third, StartupOutcomeSnapshot::Cleaning { .. })
        && matches!(terminal, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(_)} if *error == primary)
        && terminal == last
        && free
        && source
        && count == 3;
    let artifact = serde_json::json!({"scope":"real Engine input with injected thread creation failure and retryable host closer", "pending":format!("{pending:?}"), "held_first":held_first, "failed":format!("{failed:?}"), "stable":failed==repeated, "retry_advertised":advertised, "first_count":first_count, "pending_second":format!("{pending_second:?}"), "joined":format!("{joined:?}"), "held_second":held_second, "second_retryable":second_retryable, "terminal":format!("{terminal:?}"), "repeat_equal":terminal==last, "host_free":free, "creation_source_retained":source, "close_calls":count, "progress_retry_allowed":observer.snapshot().allowed_actions.retry, "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("owned-start-host-retry.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid);
}

#[cfg(feature = "dev-tools")]
fn terminated_owned_executor_process_retry(root: &Path, with_worker: bool, unfenced: bool) {
    use uc_observability_runtime::{
        DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
        ProcessObservabilityRuntime,
    };
    std::fs::create_dir_all(root).expect("root");
    let observability = ProcessObservabilityRuntime::install(ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    ))
    .expect("install")
    .handle();
    let path = root.join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (engine, events) = runtime
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .expect("engine");
    let engine = Arc::new(engine);
    let closing = Arc::clone(&engine);
    let first = runtime.spawn(async move { closing.shutdown_until_released().await });
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("first close");
    release.notify_one();
    let first_result = runtime.block_on(first).expect("shutdown task");
    let first_failed = matches!(first_result, ResourceReleaseOutcome::Failed { .. });
    let held_before = !available(&path);
    let worker_path = root.join("worker.lease");
    let mut worker_release = None;
    if with_worker {
        let lease = File::create(&worker_path).expect("worker lease");
        lease.lock().expect("worker lock");
        let (entered, waiting) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        assert!(engine.probe_spawn_owned_blocking(move || {
            entered.send(()).expect("worker entered");
            gate.recv().expect("worker release");
            drop(lease);
        }));
        waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("worker started");
        worker_release = Some(release);
    }
    if unfenced {
        engine.probe_unfenced_executor_panic();
    } else {
        engine.probe_fail_owned_executor();
    }
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !(if unfenced {
        engine.probe_unfenced_executor_panic_caught()
    } else if with_worker {
        engine.probe_owned_executor_stopping()
    } else {
        engine.probe_owned_executor_failed()
    }) {
        assert!(
            std::time::Instant::now() < until,
            "executor did not terminate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let admission_blocked = !unfenced || !engine.probe_owned_executor_admission();
    drop(events);
    drop(engine);
    let initial = observability.test_process_release(if with_worker {
        Duration::from_millis(300)
    } else {
        Duration::from_secs(10)
    });
    let worker_held = !with_worker || !available(&worker_path);
    if let Some(release) = worker_release {
        release.send(()).expect("release worker");
    }
    let after_worker = observability.test_process_release(Duration::from_secs(10));
    let worker_free = !with_worker || available(&worker_path);
    let retry = observability.test_retry_process_release(Duration::from_millis(500));
    let retry_called_host = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    if retry_called_host {
        release.notify_one();
    }
    let final_result = observability.test_process_release(Duration::from_secs(10));
    let repeated = observability.test_process_release(Duration::ZERO);
    let held_after = !available(&path);
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid = first_failed
        && held_before
        && initial == if with_worker { "pending" } else { "failed" }
        && admission_blocked
        && worker_held
        && worker_free
        && after_worker == "failed"
        && retry.0 == "failed"
        && !retry_called_host
        && final_result == "failed"
        && repeated == "failed"
        && held_after
        && count == 1;
    let artifact = serde_json::json!({"scope":"real retained Engine and process owner, controlled owned-executor panic and independent host lock", "first_result":format!("{first_result:?}"), "held_before":held_before, "worker_injected":with_worker, "unfenced_panic":unfenced, "admission_blocked":admission_blocked, "initial":initial, "worker_held":worker_held, "worker_free":worker_free, "after_worker":after_worker, "retry":format!("{retry:?}"), "retry_called_host":retry_called_host, "final":final_result, "repeated":repeated, "held_after":held_after, "close_calls":count, "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join(if unfenced {
                "terminated-owned-process-unexpected.json"
            } else if with_worker {
                "terminated-owned-process-pending.json"
            } else {
                "terminated-owned-process-retry.json"
            }),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid);
}

#[cfg(feature = "dev-tools")]
fn terminated_owned_executor_engine_retry(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    let path = root.join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temp"),
            root.join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(RetryClipboard(Mutex::new(Some(RetryStream {
            panic_on_close: false,
            lease: Some(lease),
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let (engine, events) = runtime
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .expect("engine");
    let engine = Arc::new(engine);
    let closing = Arc::clone(&engine);
    let first = runtime.spawn(async move { closing.shutdown_until_released().await });
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("first close");
    release.notify_one();
    let first_result = runtime.block_on(first).expect("first result");
    let held_before = !available(&path);
    engine.probe_fail_owned_executor();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !engine.probe_owned_executor_failed() {
        assert!(
            std::time::Instant::now() < until,
            "executor did not terminate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let closing = Arc::clone(&engine);
    let second = runtime.spawn(async move { closing.shutdown_until_released().await });
    let called_again = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    if called_again {
        release.notify_one();
    }
    let second_result = runtime.block_on(second).expect("second result");
    let repeated = runtime.block_on(engine.shutdown_with_outcome(Duration::from_millis(100)));
    let held_after = !available(&path);
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid = matches!(first_result, ResourceReleaseOutcome::Failed { .. })
        && held_before
        && !called_again
        && matches!(second_result, ResourceReleaseOutcome::Failed { .. })
        && matches!(repeated, ResourceReleaseOutcome::Failed { .. })
        && held_after
        && count == 1;
    let artifact = serde_json::json!({"scope":"real Engine, controlled owned-executor panic and independent host lock", "first":format!("{first_result:?}"),"second":format!("{second_result:?}"),"repeated":format!("{repeated:?}"),"held_before":held_before,"held_after":held_after,"called_again":called_again,"close_calls":count,"valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("terminated-owned-engine-retry.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    drop(events);
    assert!(valid);
}

fn main() {
    let root = tempfile::tempdir().expect("root");
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--terminated-active-lifecycle") {
        terminated_active_lifecycle(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--terminated-owned-engine-retry") {
        terminated_owned_executor_engine_retry(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--terminated-owned-process-unexpected") {
        terminated_owned_executor_process_retry(root.path(), true, true);
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--terminated-owned-process-pending") {
        terminated_owned_executor_process_retry(root.path(), true, false);
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--terminated-owned-process-retry") {
        terminated_owned_executor_process_retry(root.path(), false, false);
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--owned-start-host-retry") {
        owned_start_failure_host_retry(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    for arg in std::env::args() {
        let stage = match arg.as_str() {
            "--owned-start-thread" => Some("thread"),
            "--owned-start-runtime" => Some("runtime"),
            _ => None,
        };
        if let Some(stage) = stage {
            owned_start_failure(root.path(), stage);
            return;
        }
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--terminated-owned-waiter") {
        terminated_shutdown_waiter(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--delivered-owned-executor") {
        delivered_engine_executor_loss(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--independent-startup-executor") {
        independent_registration_cleanup(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    for arg in std::env::args() {
        let ending = match arg.as_str() {
            "--log-begin-success" => Some("success"),
            "--log-begin-error" => Some("error"),
            "--log-begin-panic" => Some("panic"),
            _ => None,
        };
        if let Some(ending) = ending {
            failed_log_begin_after_runtime(root.path(), ending);
            return;
        }
    }
    #[cfg(feature = "dev-tools")]
    for arg in std::env::args() {
        let ending = match arg.as_str() {
            "--undelivered-executor-success" => Some("success"),
            "--undelivered-executor-error" => Some("error"),
            "--undelivered-executor-panic" => Some("panic"),
            "--undelivered-worker-thread" => Some("thread"),
            "--undelivered-worker-executor" => Some("executor"),
            _ => None,
        };
        if let Some(ending) = ending {
            undelivered_engine_executor_loss(root.path(), ending);
            return;
        }
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--real-rollback-task-panic") {
        real_startup_rollback_task_failure(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    for arg in std::env::args() {
        let ending = match arg.as_str() {
            "--retry-worker-success" => Some("success"),
            "--retry-worker-error" => Some("error"),
            "--retry-worker-panic" => Some("panic"),
            "--retry-worker-init-panic" => Some("init-panic"),
            "--retry-worker-drop-panic" => Some("drop-panic"),
            "--retry-worker-drop-error" => Some("drop-error"),
            "--retry-worker-drop-callback" => Some("drop-callback"),
            _ => None,
        };
        if let Some(ending) = ending {
            host_retry_blocking_worker(root.path(), ending);
            return;
        }
    }
    #[cfg(feature = "dev-tools")]
    if std::env::args().any(|arg| arg == "--registered-backup-host-retry") {
        host_retry_after_backup_registration(root.path());
        return;
    }
    #[cfg(feature = "dev-tools")]
    for arg in std::env::args() {
        let ending = match arg.as_str() {
            "--detached-host-close" => Some("success"),
            "--detached-host-close-error" => Some("error"),
            "--detached-host-close-panic" => Some("panic"),
            _ => None,
        };
        if let Some(ending) = ending {
            startup_detached_host_close(root.path(), ending);
            return;
        }
    }
    for arg in std::env::args() {
        let options = match arg.as_str() {
            "--worker-thread-backup" => Some(("thread", false)),
            "--worker-executor-backup" => Some(("executor", false)),
            "--worker-panic-backup" => Some(("panic", false)),
            "--worker-backup-settled" => Some(("thread", true)),
            _ => None,
        };
        if let Some((kind, settle_first)) = options {
            unpolled_failure_with_backup(root.path(), kind, settle_first);
            return;
        }
    }
    for arg in std::env::args() {
        let options = match arg.as_str() {
            "--host-first-backup" => Some((false, true)),
            "--backup-first-host" => Some((true, true)),
            "--backup-first-host-success" => Some((true, false)),
            "--host-success-first-backup" => Some((false, false)),
            _ => None,
        };
        if let Some((backup_first, host_fails)) = options {
            concurrent_host_and_backup(root.path(), backup_first, host_fails);
            return;
        }
    }
    for arg in std::env::args() {
        let options = match arg.as_str() {
            "--rollback-backup" => Some((false, false, false)),
            "--rollback-backup-panic" => Some((true, false, false)),
            "--rollback-backup-settled" => Some((false, true, false)),
            "--host-failure-backup" => Some((false, false, true)),
            _ => None,
        };
        if let Some((panic_after_release, settle_first, fail_host)) = options {
            rollback_with_active_backup(root.path(), panic_after_release, settle_first, fail_host);
            return;
        }
    }
    let abandoned = abandoned_input();
    let unpolled_progress = unpolled_progress_after_cleanup(&root.path().join("unpolled-progress"));
    let host_retry = host_close_retry(&root.path().join("host-retry"));
    let host_panic = host_close_panic(&root.path().join("host-panic"));
    let construction_backup =
        host_close_with_active_backup(&root.path().join("construction-backup"), false);
    let construction_backup_panic =
        host_close_with_active_backup(&root.path().join("construction-backup-panic"), true);
    let construction_close =
        host_close_before_construction_settles(&root.path().join("construction-close"));
    let executor_host_retry =
        host_close_retry_with_executor(&root.path().join("executor-host-retry"));
    let released = scenario(&root.path().join("released"), false);
    let failed = scenario(&root.path().join("failed"), true);
    let delivered = delivered_engine(&root.path().join("delivered"));
    let backup = executor_loss_during_backup(&root.path().join("backup"), false);
    let backup_failure = executor_loss_during_backup(&root.path().join("backup-failure"), true);
    let late_registration =
        executor_loss_during_registration(&root.path().join("late-registration"));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("startup-owner-result.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production Engine startup modules, controlled host closers, independent host lock probes", "abandoned_input":abandoned, "unpolled_progress":unpolled_progress, "host_retry":host_retry, "host_panic":host_panic, "construction_close":construction_close, "construction_backup":construction_backup, "construction_backup_panic":construction_backup_panic, "executor_host_retry":executor_host_retry, "released":released, "failed":failed, "delivered":delivered, "backup":backup, "backup_failure":backup_failure, "late_registration":late_registration, "full_startup_release":"not established", "device_acceptance":"not established"})).expect("json")).expect("artifact");
    }
}
