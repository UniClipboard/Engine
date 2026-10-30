//! 真实 Engine 与受控宿主关闭失败；不代表移动设备验收。
use std::collections::HashMap;
use std::fs::File;
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, EngineEvent, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboard, HostClipboardChange, HostClipboardChangeStream,
    HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    HostSecureStorage, Operation, OperationTerminal, ResourceReleaseOutcome, SecretString,
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
struct Gate {
    enabled: std::sync::atomic::AtomicBool,
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
struct Storage {
    values: Mutex<HashMap<String, Vec<u8>>>,
    gate: Arc<Gate>,
}
impl HostSecureStorage for Storage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        if self
            .gate
            .enabled
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.gate.entered.send(()).expect("gate entry");
            self.gate
                .release
                .lock()
                .unwrap()
                .recv()
                .expect("gate release");
        }
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.values.lock().unwrap().remove(key);
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

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let exceed = std::env::args().any(|arg| arg == "--exceed");
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("host.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let gate = Arc::new(Gate {
        enabled: AtomicBool::new(false),
        entered,
        release: Mutex::new(blocked),
    });
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(Storage {
            values: Mutex::new(HashMap::new()),
            gate: Arc::clone(&gate),
        }),
        Box::new(Clipboard(Mutex::new(Some(TestStream {
            lease,
            fail: false,
            calls: Arc::clone(&calls),
        })))),
        Box::new(Files),
    );
    let (engine, mut events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .expect("engine");
    let engine = Arc::new(engine);
    gate.enabled.store(true, Ordering::SeqCst);
    let op_engine = Arc::clone(&engine);
    let operation = tokio::spawn(async move {
        op_engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("test".into()),
                passphrase: SecretString::new("correct horse battery staple"),
                passphrase_confirmation: SecretString::new("correct horse battery staple"),
            }))
            .await
    });
    tokio::task::spawn_blocking(move || ready.recv_timeout(Duration::from_secs(15)))
        .await
        .expect("join")
        .expect("operation entered storage");
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
    let grace = if exceed {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(5)
    };
    sender
        .send(worker_lifecycle::LifecycleCommand::Shutdown {
            deadline: Instant::now() + grace,
            response: reply,
        })
        .expect("shutdown");
    let cancelled_early = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            match events.next().await {
                Some(EngineEvent::OperationFinished {
                    terminal: OperationTerminal::Cancelled,
                    ..
                }) => break true,
                Some(_) => {}
                None => break false,
            }
        }
    })
    .await
    .unwrap_or(false);
    let held = !available(&path);
    release.send(()).expect("release gate");
    let operation_result = tokio::time::timeout(Duration::from_secs(20), operation)
        .await
        .expect("operation timeout")
        .expect("operation join");
    let shutdown =
        tokio::task::spawn_blocking(move || result.recv_timeout(Duration::from_secs(20)))
            .await
            .expect("join")
            .expect("shutdown result");
    drop(sender);
    tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .expect("worker timeout")
        .expect("worker join");
    let count = calls.load(Ordering::SeqCst);
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        let name = if exceed {
            "binding-operation-grace-expired.json"
        } else {
            "binding-operation-grace.json"
        };
        std::fs::write(std::path::Path::new(&dir).join(name), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine create-space operation with gated secure storage and production binding lifecycle loop", "short_grace":exceed, "cancelled_during_observation":cancelled_early, "host_lock_held_during_operation":held, "operation_succeeded":operation_result.is_ok(), "shutdown_released":matches!(shutdown, BindingResourceRelease::Released { .. }), "close_calls":count, "actual_mobile_host":"not exercised"})).expect("json")).expect("artifact");
    }
    assert_eq!(
        cancelled_early, exceed,
        "operation cancellation did not follow the shutdown budget"
    );
    assert!(held && matches!(shutdown, BindingResourceRelease::Released { .. }) && count == 1);
}
