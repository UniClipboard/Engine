use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::timeout;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput, HostCapabilities,
    HostCapabilityError, HostDirectories, HostSecureStorage, Operation, OperationResult,
    SecretString, SendTextInput, StartupProgress, StartupState,
};

use super::{EmptyClipboard, EmptyFiles, MemorySecureStorage};

#[path = "startup/crash.rs"]
mod crash;

#[path = "startup/targets.rs"]
mod targets;

#[path = "startup/control.rs"]
mod control;

#[cfg(feature = "dev-tools")]
#[path = "startup/failure.rs"]
mod failure;

struct HeldSecureStorage {
    storage: MemorySecureStorage,
    entered: Arc<Notify>,
    release: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
}

impl HostSecureStorage for HeldSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        if key.starts_with("kek:v1:") {
            if let Some(release) = self.release.lock().unwrap().take() {
                self.entered.notify_one();
                release.recv_timeout(Duration::from_secs(20)).unwrap();
            }
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

pub(super) fn host(root: &Path, storage: Box<dyn HostSecureStorage>) -> HostCapabilities {
    HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        storage,
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_startup_finishes_cleanup_before_retry_and_preserves_saved_content() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("startup cancellation test".into()),
            passphrase: SecretString::new("startup-test-passphrase"),
            passphrase_confirmation: SecretString::new("startup-test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(sent) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "content saved before startup cancellation".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved entry");
    };
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (release, released) = mpsc::channel();
    let entered = Arc::new(Notify::new());
    let blocked_host = host(
        root.path(),
        Box::new(HeldSecureStorage {
            storage: storage.clone(),
            entered: Arc::clone(&entered),
            release: Arc::new(Mutex::new(Some(released))),
        }),
    );
    let (input, mut progress) = StartupProgress::channel();
    let caller = tokio::spawn(Engine::start_with_progress(
        EngineConfig::new("2.0.0"),
        blocked_host,
        input,
    ));
    timeout(Duration::from_secs(15), entered.notified())
        .await
        .unwrap();
    caller.abort();
    tokio::task::yield_now().await;
    let retry_before_cleanup = progress.snapshot().allowed_actions.retry;
    release.send(()).unwrap();
    assert!(caller.await.err().unwrap().is_cancelled());
    assert!(!retry_before_cleanup);
    timeout(Duration::from_secs(20), async {
        while !progress.snapshot().state.is_terminal() {
            progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(progress.snapshot().state, StartupState::Interrupted);
    assert!(progress.snapshot().allowed_actions.retry);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    let OperationResult::HistoryEntry(entry) = engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: sent.entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved history");
    };
    assert_eq!(entry.content, "content saved before startup cancellation");
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}
