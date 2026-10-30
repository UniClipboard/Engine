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

#[allow(unexpected_cfgs, reason = "直接复用绑定模块；测试特性属于绑定 crate")]
mod startup_delivery {
    include!("../../../bindings/uc-engine-uniffi/src/runtime/startup_delivery.rs");

    pub(super) fn failed_result(owner: &StartupDeliveryOwner) -> bool {
        let state = owner
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(
            state
                .as_ref()
                .and_then(|retained| retained.cleanup_result.as_ref()),
            Some(BindingResourceRelease::Failed { .. })
        )
    }
}

type BindingError = uc_engine::EngineError;
#[allow(dead_code)]
#[derive(Clone)]
enum BindingResourceRelease {
    Pending,
    Released { error: Option<BindingError> },
    Failed { error: BindingError },
}

impl From<ResourceReleaseOutcome> for BindingResourceRelease {
    fn from(outcome: ResourceReleaseOutcome) -> Self {
        match outcome {
            ResourceReleaseOutcome::Pending => Self::Pending,
            ResourceReleaseOutcome::Released { error } => Self::Released { error },
            ResourceReleaseOutcome::Failed { error } => Self::Failed { error },
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
    worker_lease: Option<File>,
    entered: Option<std::sync::mpsc::Sender<()>>,
    release: Option<std::sync::mpsc::Receiver<()>>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl HostClipboardChangeStream for TestStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let lease = self.worker_lease.take().expect("worker lease");
        let entered = self.entered.take().expect("worker entry");
        let release = self.release.take().expect("worker gate");
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            entered.send(()).expect("worker entry");
            release.recv().expect("worker gate");
        });
        let _ = self.lease.metadata();
        panic!("controlled host close panic after blocking work was accepted");
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    let root = tempfile::tempdir().expect("root");
    let host_path = root.path().join("host.lease");
    let host_lease = File::create(&host_path).expect("host lease");
    host_lease.lock().expect("host lock");
    let worker_path = root.path().join("worker.lease");
    let worker_lease = File::create(&worker_path).expect("worker lease");
    worker_lease.lock().expect("worker lock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered, waiting) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(Storage::default()),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease: host_lease,
            worker_lease: Some(worker_lease),
            entered: Some(entered),
            release: Some(gate),
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let (engine, events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("engine");
    let engine = Arc::new(engine);
    let owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    owner.accept(Arc::clone(&engine));
    let weak = Arc::downgrade(&owner);
    let worker_owner = Arc::clone(&owner);
    let closing = tokio::spawn(async move {
        worker_owner
            .shutdown_owned(
                engine,
                None,
                uc_engine::EngineError::new(1108, uc_engine::EngineErrorCategory::Internal, false),
            )
            .await
    });
    tokio::task::spawn_blocking(move || waiting.recv_timeout(Duration::from_secs(15)))
        .await
        .expect("join")
        .expect("blocking worker entered");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let pending = !closing.is_finished();
    let host_held = !available(&host_path);
    let worker_held = !available(&worker_path);
    release.send(()).expect("release worker");
    let outcome = tokio::time::timeout(Duration::from_secs(15), closing)
        .await
        .expect("shutdown settled")
        .expect("shutdown join");
    let failed = matches!(outcome, ResourceReleaseOutcome::Failed { .. });
    let binding = BindingResourceRelease::from(outcome);
    owner.finish(&binding);
    drop(events);
    drop(owner);
    let retained = weak.upgrade().is_some();
    let worker_free = available(&worker_path);
    let host_held_after = !available(&host_path);
    let count = calls.load(Ordering::SeqCst);
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(std::path::Path::new(&dir).join("binding-cleanup-task-panic.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine, production binding owned cleanup, panicking host close task, accepted blocking worker and independent locks", "pending_while_worker_held":pending, "host_lock_held":host_held, "worker_lock_held":worker_held, "failed_after_worker":failed, "owner_retained":retained, "worker_lock_released":worker_free, "host_lock_held_after":host_held_after, "close_calls":count, "cleanup_thread_panic":"not exercised", "native_mobile":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        pending
            && host_held
            && worker_held
            && failed
            && retained
            && worker_free
            && host_held_after
            && count == 1
    );
}
