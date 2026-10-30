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
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    fail: bool,
}
#[async_trait]
impl HostClipboardChangeStream for TestStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let fail = std::env::args().any(|arg| arg == "--fail");
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, accepted) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
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
            entered: Mutex::new(Some(entered)),
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
            fail,
        })))),
        Box::new(Files),
    );
    let (engine, events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("engine");
    let owner = Arc::new(startup_delivery::StartupDeliveryOwner::default());
    let weak = Arc::downgrade(&owner);
    let worker_owner = Arc::clone(&owner);
    let worker = std::thread::spawn(move || {
        let _guard = startup_delivery::StartupWorkerGuard::new(
            Arc::clone(&worker_owner),
            uc_engine::EngineError::new(4242, uc_engine::EngineErrorCategory::Internal, false),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("binding runtime");
        let engine = Arc::new(engine);
        worker_owner.accept(Arc::clone(&engine));
        runtime.block_on(async move {
            let close = tokio::spawn(async move {
                worker_owner
                    .shutdown_owned(
                        engine,
                        None,
                        uc_engine::EngineError::new(
                            1108,
                            uc_engine::EngineErrorCategory::Internal,
                            false,
                        ),
                    )
                    .await
            });
            accepted.await.expect("closer accepted");
            drop(close);
            panic!("controlled worker panic during accepted shutdown");
        });
    });
    let panicked = tokio::task::spawn_blocking(move || worker.join().is_err())
        .await
        .expect("worker join");
    let held_before = !available(&path);
    let calls_before = calls.load(Ordering::SeqCst);
    drop(events);
    let retained_before = weak.upgrade().is_some();
    let waiting_owner = Arc::clone(&owner);
    let long_wait =
        tokio::task::spawn_blocking(move || waiting_owner.wait_internal(Duration::from_secs(15)));
    let short_pending = matches!(
        owner.wait_internal(Duration::from_millis(100)),
        startup_delivery::DeliveryOutcome::Pending
    );
    release.notify_one();
    let long_outcome = long_wait.await.expect("long waiter");
    let error_codes = match &long_outcome {
        startup_delivery::DeliveryOutcome::Failed {
            error,
            worker_error: Some(worker_error),
            ..
        } => Some((error.code(), worker_error.code())),
        _ => None,
    };
    let long_correct = if fail {
        matches!(
            long_outcome,
            startup_delivery::DeliveryOutcome::Failed {
                worker_error: Some(_),
                ..
            }
        )
    } else {
        matches!(
            long_outcome,
            startup_delivery::DeliveryOutcome::Released { error: None, worker_error: Some(ref error) } if error.code() == 4242
        )
    };
    let repeated = owner.wait_internal(Duration::ZERO);
    let repeated_error_codes = match &repeated {
        startup_delivery::DeliveryOutcome::Failed {
            error,
            worker_error: Some(worker_error),
            ..
        } => Some((error.code(), worker_error.code())),
        _ => None,
    };
    let repeated_correct = if fail {
        matches!(
            repeated,
            startup_delivery::DeliveryOutcome::Failed {
                worker_error: Some(_),
                ..
            }
        )
    } else {
        matches!(
            repeated,
            startup_delivery::DeliveryOutcome::Released { error: None, worker_error: Some(ref error) } if error.code() == 4242
        )
    };
    drop(owner);
    let deadline = Instant::now() + Duration::from_secs(15);
    while weak
        .upgrade()
        .is_some_and(|owner| !fail || !startup_delivery::failed_result(&owner))
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let released_owner = weak.upgrade().is_none();
    let settled_failed = weak
        .upgrade()
        .is_some_and(|owner| startup_delivery::failed_result(&owner));
    let released_lock = available(&path);
    let final_calls = calls.load(Ordering::SeqCst);
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        let name = if fail {
            "binding-accepted-close-panic-failed.json"
        } else {
            "binding-accepted-close-panic.json"
        };
        std::fs::write(std::path::Path::new(&dir).join(name), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production binding delivery guard, simulated worker executor panic, real Engine and gated host close with independent lock", "worker_panicked":panicked, "lock_held_before_gate":held_before, "owner_retained_before_gate":retained_before, "short_wait_pending":short_pending, "long_wait_correct":long_correct, "repeated_wait_correct":repeated_correct, "cleanup_and_worker_error_codes":error_codes, "calls_before_gate":calls_before, "closer_failed":fail, "failed_result_settled":settled_failed, "owner_released":released_owner, "lock_released":released_lock, "final_close_calls":final_calls, "actual_binding_worker":"not exercised", "native_mobile":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        panicked
            && held_before
            && retained_before
            && short_pending
            && long_correct
            && repeated_correct
            && (!fail || error_codes == Some((1108, 4242)))
            && error_codes == repeated_error_codes
            && calls_before == 1
            && settled_failed == fail
            && released_owner == !fail
            && released_lock == !fail
            && final_calls == 1,
        "accepted close did not settle after the worker terminated"
    );
}
