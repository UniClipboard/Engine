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

#[path = "../../../bindings/uc-engine-uniffi/src/runtime/worker_lifecycle.rs"]
mod worker_lifecycle;
mod shutdown_wait {
    use std::sync::mpsc::Sender;

    use crate::BindingResourceRelease;

    pub(super) type ShutdownReply = Sender<BindingResourceRelease>;
}
mod observability {
    pub(super) fn schedule_flush_after_lifecycle() {}
}
type BindingEngineState = uc_engine::EngineState;
fn map_engine_state(state: uc_engine::EngineState) -> BindingEngineState {
    state
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
    fn from(value: ResourceReleaseOutcome) -> Self {
        match value {
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
    fail: bool,
    calls: Arc<std::sync::atomic::AtomicUsize>,
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
    let retry = std::env::args().any(|arg| arg == "--retry");
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
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
            fail: true,
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let (engine, events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("engine");
    let engine = Arc::new(engine);
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (reply, result) = std::sync::mpsc::channel();
    let worker = tokio::spawn(async move {
        let (replies, outcome) =
            worker_lifecycle::run(engine, receiver, |engine, grace| async move {
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
            deadline: Instant::now() + Duration::from_secs(10),
            response: reply,
        })
        .expect("send");
    let outcome = tokio::task::spawn_blocking(move || result.recv_timeout(Duration::from_secs(15)))
        .await
        .expect("join")
        .expect("result");
    let first = calls.load(Ordering::SeqCst);
    let retry_failed = if retry {
        let (reply, result) = std::sync::mpsc::channel();
        sender
            .send(worker_lifecycle::LifecycleCommand::Shutdown {
                deadline: Instant::now() + Duration::from_secs(10),
                response: reply,
            })
            .expect("send retry");
        matches!(
            tokio::task::spawn_blocking(move || result.recv_timeout(Duration::from_secs(15)))
                .await
                .expect("retry join")
                .expect("retry result"),
            BindingResourceRelease::Failed { .. }
        )
    } else {
        true
    };
    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .expect("worker")
        .expect("worker join");
    let final_calls = calls.load(Ordering::SeqCst);
    let held = !available(&path);
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        let name = if retry {
            "binding-lifecycle-explicit-retry.json"
        } else {
            "binding-lifecycle-failed-close.json"
        };
        std::fs::write(std::path::Path::new(&dir).join(name), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production binding lifecycle loop with a real Engine, controlled retryable host closer and independent lock", "first_failed":matches!(outcome, BindingResourceRelease::Failed { .. }), "calls_at_failure":first, "explicit_retry":retry, "retry_failed":retry_failed, "calls_after_channel_loss":final_calls, "lock_held":held, "actual_mobile_host":"not exercised"})).expect("json")).expect("artifact");
    }
    drop(events);
    assert!(
        matches!(outcome, BindingResourceRelease::Failed { .. })
            && first == 1
            && retry_failed
            && final_calls == if retry { 2 } else { 1 }
            && held,
        "failed close must not be retried implicitly: {first} -> {final_calls}"
    );
}
