use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::{sleep, timeout};
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, EngineState, HostCapabilities, HostCapabilityError,
    HostClipboard, HostClipboardChange, HostClipboardChangeStream, HostClipboardRepresentation,
    HostClipboardSnapshot, HostDirectories, Operation, OperationResult, QueryHistoryInput,
    SecretString, SendTextInput,
};

use super::{find_lease, open_lease, EmptyFiles, MemorySecureStorage};

struct NotifyingClipboard {
    changes: Mutex<Option<Box<dyn HostClipboardChangeStream>>>,
}

impl HostClipboard for NotifyingClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 42,
            representations: vec![HostClipboardRepresentation::Inline {
                format: "text".into(),
                mime_type: Some("text/plain".into()),
                bytes: b"stale clipboard callback".to_vec(),
            }],
        })
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self.changes.lock().unwrap().take())
    }
}

struct ClipboardChanges {
    receiver: tokio::sync::mpsc::UnboundedReceiver<()>,
    stopped: Arc<AtomicBool>,
}

#[async_trait]
impl HostClipboardChangeStream for ClipboardChanges {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        Ok(match self.receiver.recv().await {
            Some(()) => HostClipboardChange::Changed,
            None => HostClipboardChange::Closed,
        })
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.stopped.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspended_engine_keeps_old_clipboard_callbacks_silent_after_profile_release() {
    let root = tempfile::tempdir().unwrap();
    let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(NotifyingClipboard {
            changes: Mutex::new(Some(Box::new(ClipboardChanges {
                receiver: change_rx,
                stopped: Arc::clone(&stopped),
            }))),
        }),
        Box::new(EmptyFiles),
    );
    let (engine, _events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("stale callback test".into()),
            passphrase: SecretString::new("stale-callback-passphrase"),
            passphrase_confirmation: SecretString::new("stale-callback-passphrase"),
        }))
        .await
        .unwrap();
    let lease = find_lease(root.path()).unwrap();

    timeout(Duration::from_secs(10), engine.suspend())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    assert!(!stopped.load(Ordering::SeqCst));
    let other_owner = open_lease(&lease);
    other_owner.try_lock().unwrap();
    change_tx.send(()).unwrap();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    drop(other_owner);

    engine.resume().await.unwrap();
    sleep(Duration::from_secs(1)).await;
    let OperationResult::HistoryPage { entries, .. } = engine
        .execute(Operation::QueryHistory(QueryHistoryInput {
            cursor: None,
            limit: 10,
            query: None,
        }))
        .await
        .unwrap()
    else {
        panic!("expected history page");
    };
    assert!(entries.is_empty(), "旧通知不得在暂停后补写历史");
    engine
        .execute(Operation::SendText(SendTextInput {
            text: "fresh work after resume".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap();
    let OperationResult::HistoryPage { entries, .. } = engine
        .execute(Operation::QueryHistory(QueryHistoryInput {
            cursor: None,
            limit: 10,
            query: None,
        }))
        .await
        .unwrap()
    else {
        panic!("expected history page");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].preview.as_deref(),
        Some("fresh work after resume")
    );
    engine.shutdown_until_complete().await.unwrap();
    assert!(stopped.load(Ordering::SeqCst));
}
