//! 真实 Engine 与受控宿主关闭失败；不代表移动设备验收。
use std::collections::HashMap;
use std::fs::File;
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_engine::{
    Engine, EngineConfig, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardChange, HostClipboardChangeStream, HostClipboardSnapshot,
    HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage,
    ResourceReleaseOutcome,
};

#[path = "../../../bindings/uc-engine-uniffi/src/runtime/shutdown_wait.rs"]
mod shutdown_wait;
#[path = "../../../bindings/uc-engine-uniffi/src/runtime/worker_lifecycle.rs"]
#[allow(dead_code, reason = "Probe uses a subset of this module")]
mod worker_lifecycle;
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
mod observability {
    pub(super) fn schedule_flush_after_lifecycle() {}
}
type BindingEngineState = uc_engine::EngineState;
fn map_engine_state(state: uc_engine::EngineState) -> BindingEngineState {
    state
}

#[derive(Clone)]
enum BindingError {
    RuntimeUnavailable,
    Engine,
}
impl From<uc_engine::EngineError> for BindingError {
    fn from(_: uc_engine::EngineError) -> Self {
        Self::Engine
    }
}
#[allow(dead_code)]
#[derive(Clone)]
enum BindingResourceRelease {
    Pending,
    Released { error: Option<BindingError> },
    Failed { error: BindingError },
}

impl From<ResourceReleaseOutcome> for BindingResourceRelease {
    fn from(value: ResourceReleaseOutcome) -> Self {
        match value {
            ResourceReleaseOutcome::Pending => Self::Pending,
            ResourceReleaseOutcome::Released { error } => Self::Released {
                error: error.map(Into::into),
            },
            ResourceReleaseOutcome::Failed { error } => Self::Failed {
                error: error.into(),
            },
        }
    }
}
#[derive(Default)]
struct Storage(Mutex<HashMap<String, Vec<u8>>>);
impl HostSecureStorage for Storage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
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
        "controlled failure",
    )
}

struct TestStream {
    lease: File,
    fail: bool,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    entered: std::sync::mpsc::Sender<()>,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl HostClipboardChangeStream for TestStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    fn supports_shutdown_retry(&self) -> bool {
        true
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = self.entered.send(());
        self.release.notified().await;
        let _ = self.lease.metadata();
        if self.fail {
            Err(unavailable())
        } else {
            Ok(())
        }
    }
}

struct Clipboard(Mutex<Option<TestStream>>);
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
            .unwrap()
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

fn available(path: &std::path::Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("independent lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    use shutdown_wait::{ShutdownReply, ShutdownWait};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let (entered, started) = std::sync::mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease,
            fail: true,
            calls: Arc::clone(&calls),
            entered,
            release: Arc::clone(&release),
        })))),
        Box::new(Files),
    );
    let (engine, events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("engine");
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let waiting = Arc::new(ShutdownWait::default());
    let worker = tokio::spawn(async move {
        let (replies, outcome) =
            worker_lifecycle::run(Arc::new(engine), receiver, |engine, grace| async move {
                match grace {
                    Some(grace) => engine.shutdown_until_released_with_grace(grace).await,
                    None => engine.shutdown_until_released().await,
                }
            })
            .await;
        for reply in replies {
            let _ = reply.send(BindingResourceRelease::from(outcome.clone()));
        }
    });
    sender
        .send(worker_lifecycle::LifecycleCommand::Shutdown {
            deadline: Instant::now(),
            response: ShutdownReply(Arc::clone(&waiting)),
        })
        .expect("send");
    tokio::task::spawn_blocking(move || started.recv_timeout(Duration::from_secs(15)))
        .await
        .expect("join")
        .expect("close started");
    let long_waiter = Arc::clone(&waiting);
    let (waiter_started, waiter_ready) = std::sync::mpsc::channel();
    let long_wait = tokio::task::spawn_blocking(move || {
        waiter_started.send(()).expect("waiter started");
        long_waiter.wait(Instant::now() + Duration::from_secs(15))
    });
    tokio::task::spawn_blocking(move || waiter_ready.recv_timeout(Duration::from_secs(5)))
        .await
        .expect("waiter join")
        .expect("waiter ready");
    let first_pending = matches!(
        waiting.wait(Instant::now() + Duration::from_millis(100)),
        BindingResourceRelease::Pending
    );
    let second_pending = matches!(
        waiting.wait(Instant::now()),
        BindingResourceRelease::Pending
    );
    let lock_held_pending = !available(&path);
    let pending_calls = calls.load(Ordering::SeqCst);
    let long_wait_pending = !long_wait.is_finished();
    release.notify_one();
    let waiter = Arc::clone(&waiting);
    let outcome =
        tokio::task::spawn_blocking(move || waiter.wait(Instant::now() + Duration::from_secs(15)))
            .await
            .expect("wait join");
    let failed = matches!(outcome, BindingResourceRelease::Failed { .. });
    let concurrent_failed = matches!(
        long_wait.await.expect("long wait"),
        BindingResourceRelease::Failed { .. }
    );
    let repeated_failed = matches!(
        waiting.wait(Instant::now()),
        BindingResourceRelease::Failed { .. }
    );
    drop(sender);
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .expect("worker")
        .expect("worker join");
    let final_calls = calls.load(Ordering::SeqCst);
    let lock_held_failed = !available(&path);
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(std::path::Path::new(&dir).join("binding-lifecycle-deadline.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production binding lifecycle and shutdown wait modules, real Engine, controlled host closer and independent lock", "first_pending":first_pending, "second_pending":second_pending, "long_wait_pending":long_wait_pending, "lock_held_pending":lock_held_pending, "pending_calls":pending_calls, "failed":failed, "concurrent_failed":concurrent_failed, "repeated_failed":repeated_failed, "final_calls":final_calls, "lock_held_failed":lock_held_failed, "actual_mobile_host":"not exercised"})).expect("json")).expect("artifact");
    }
    drop(events);
    assert!(
        first_pending
            && second_pending
            && long_wait_pending
            && lock_held_pending
            && pending_calls == 1
            && failed
            && concurrent_failed
            && repeated_failed
            && final_calls == 1
            && lock_held_failed
    );
}
