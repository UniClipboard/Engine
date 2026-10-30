//! 隔离宿主的完整 Engine 剪贴板归因验收；不访问系统剪贴板。

use std::sync::{Arc, Mutex};

use uc_engine::{
    ClipboardRestoreMode, CreateSpaceInput, Engine, EngineConfig, HostCapabilities,
    HostCapabilityError, HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot,
    HostDirectories, ObserveClipboardChangeInput, Operation, OperationResult,
    RestoreClipboardInput, SecretString, SendTextInput,
};

use super::{EmptyFiles, MemorySecureStorage};

#[derive(Clone, Default)]
struct Clipboard(Arc<Mutex<Vec<HostClipboardRepresentation>>>);

impl Clipboard {
    fn copy_text(&self, text: &str) {
        *self.0.lock().unwrap() = vec![HostClipboardRepresentation::Inline {
            format: "text/plain".into(),
            mime_type: Some("text/plain".into()),
            bytes: text.as_bytes().to_vec(),
        }];
    }
}

impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 1,
            representations: self.0.lock().unwrap().clone(),
        })
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        *self.0.lock().unwrap() = snapshot.representations;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_user_copy_after_unobserved_restore_is_captured() {
    let root = tempfile::tempdir().unwrap();
    let clipboard = Clipboard::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(clipboard.clone()),
        Box::new(EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(entry) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "synthetic restored text".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored entry")
    };
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: entry.entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();

    // 不交付恢复产生的通知；下一次观察属于另一份真实用户复制。
    clipboard.copy_text("synthetic different user text");
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let result = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(result, OperationResult::HistoryStats(ref stats) if stats.total_items == 2),
        "a different user copy must be captured after an unobserved restore: {result:?}"
    );
}

#[derive(Clone, Default)]
struct IdentifiedClipboard(Arc<Mutex<(u64, Vec<HostClipboardRepresentation>)>>);

impl IdentifiedClipboard {
    fn copy_text(&self, text: &str) {
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        state.1 = vec![HostClipboardRepresentation::Inline {
            format: "text/plain".into(),
            mime_type: Some("text/plain".into()),
            bytes: text.as_bytes().to_vec(),
        }];
    }
}

impl HostClipboard for IdentifiedClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.read_with_identity().map(|(snapshot, _)| snapshot)
    }

    fn read_with_identity(
        &self,
    ) -> Result<
        (
            HostClipboardSnapshot,
            Option<uc_engine::HostClipboardIdentity>,
        ),
        HostCapabilityError,
    > {
        let state = self.0.lock().unwrap();
        Ok((
            HostClipboardSnapshot {
                observed_at_ms: 1,
                representations: state.1.clone(),
            },
            Some(uc_engine::HostClipboardIdentity(
                state.0.to_be_bytes().to_vec(),
            )),
        ))
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.write_with_receipt(snapshot).map(|_| ())
    }

    fn write_with_receipt(
        &self,
        snapshot: HostClipboardSnapshot,
    ) -> Result<uc_engine::HostClipboardWriteReceipt, HostCapabilityError> {
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        let _ = snapshot;
        state.1 = vec![HostClipboardRepresentation::Inline {
            format: "text/plain".into(),
            mime_type: Some("text/plain".into()),
            bytes: b"synthetic native projection".to_vec(),
        }];
        Ok(uc_engine::HostClipboardWriteReceipt {
            identity: Some(uc_engine::HostClipboardIdentity(
                state.0.to_be_bytes().to_vec(),
            )),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identified_restore_duplicates_and_new_user_copy() {
    let root = tempfile::tempdir().unwrap();
    let clipboard = IdentifiedClipboard::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(clipboard.clone()),
        Box::new(EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(entry) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "synthetic restored text".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored entry")
    };
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: entry.entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();
    for _ in 0..2 {
        engine
            .execute(Operation::ObserveClipboardChange(
                ObserveClipboardChangeInput { dispatch: false },
            ))
            .await
            .unwrap();
    }
    let after_echo = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    clipboard.copy_text("synthetic different user text");
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let after_copy = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(after_echo, OperationResult::HistoryStats(ref stats) if stats.total_items == 1)
    );
    assert!(
        matches!(after_copy, OperationResult::HistoryStats(ref stats) if stats.total_items == 2)
    );
}

#[derive(Clone)]
struct Files(std::path::PathBuf);

impl uc_engine::HostFileAccess for Files {
    fn metadata(
        &self,
        handle: &uc_engine::HostFileHandle,
    ) -> Result<uc_engine::HostFileMetadata, HostCapabilityError> {
        let bytes = std::fs::read(self.0.join(handle.as_str())).unwrap();
        Ok(uc_engine::HostFileMetadata {
            display_name: "synthetic-file".into(),
            size_bytes: bytes.len() as u64,
            mime_type: Some("application/octet-stream".into()),
        })
    }

    fn read_chunk(
        &self,
        handle: &uc_engine::HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        let bytes = std::fs::read(self.0.join(handle.as_str())).unwrap();
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(max_bytes as usize).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    fn write_chunk(
        &self,
        _: &uc_engine::HostFileHandle,
        _: u64,
        _: &[u8],
    ) -> Result<(), HostCapabilityError> {
        Ok(())
    }
    fn finish_write(&self, _: &uc_engine::HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identified_file_projection_and_later_user_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("first"), b"synthetic first file contents").unwrap();
    std::fs::write(
        root.path().join("second"),
        b"synthetic different file contents",
    )
    .unwrap();
    let clipboard = IdentifiedClipboard::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(clipboard.clone()),
        Box::new(Files(root.path().to_owned())),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(entry) = engine
        .execute(Operation::SendFiles(uc_engine::SendFilesInput {
            files: vec![uc_engine::HostFileHandle::new("first")],
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored file")
    };
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: entry.entry_id.clone(),
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let after_echo = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    // 第二次恢复不交付观察，再复制不同文件。
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: entry.entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();
    {
        let mut state = clipboard.0.lock().unwrap();
        state.0 += 1;
        state.1 = vec![HostClipboardRepresentation::File {
            format: "files".into(),
            handle: uc_engine::HostFileHandle::new("second"),
            display_name: "synthetic second file".into(),
            mime_type: Some("application/octet-stream".into()),
            size_bytes: b"synthetic different file contents".len() as u64,
        }];
    }
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let after_copy = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(after_echo, OperationResult::HistoryStats(ref stats) if stats.total_items == 1),
        "{after_echo:?}"
    );
    assert!(
        matches!(after_copy, OperationResult::HistoryStats(ref stats) if stats.total_items == 2),
        "{after_copy:?}"
    );
}

struct HeldWriteClipboard {
    inner: IdentifiedClipboard,
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl HostClipboard for HeldWriteClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.inner.read()
    }

    fn read_with_identity(
        &self,
    ) -> Result<
        (
            HostClipboardSnapshot,
            Option<uc_engine::HostClipboardIdentity>,
        ),
        HostCapabilityError,
    > {
        self.inner.read_with_identity()
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.write_with_receipt(snapshot).map(|_| ())
    }

    fn write_with_receipt(
        &self,
        snapshot: HostClipboardSnapshot,
    ) -> Result<uc_engine::HostClipboardWriteReceipt, HostCapabilityError> {
        let receipt = self.inner.write_with_receipt(snapshot)?;
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(receipt)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identified_observation_waits_for_write_completion_after_caller_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let clipboard = IdentifiedClipboard::default();
    let (entered_send, entered_recv) = std::sync::mpsc::channel();
    let (release_send, release_recv) = std::sync::mpsc::channel();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(HeldWriteClipboard {
            inner: clipboard,
            entered: entered_send,
            release: Mutex::new(release_recv),
        }),
        Box::new(EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    let engine = Arc::new(engine);
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(entry) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "synthetic restored text".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored entry")
    };
    let writer = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                    entry_id: entry.entry_id,
                    mode: ClipboardRestoreMode::Standard,
                }))
                .await
        }
    });
    tokio::task::spawn_blocking(move || {
        entered_recv.recv_timeout(std::time::Duration::from_secs(10))
    })
    .await
    .unwrap()
    .unwrap();
    writer.abort();
    let mut observed = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::ObserveClipboardChange(
                    ObserveClipboardChangeInput { dispatch: false },
                ))
                .await
        }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut observed)
            .await
            .is_err()
    );
    release_send.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), observed)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let history = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(history, OperationResult::HistoryStats(ref stats) if stats.total_items == 1),
        "{history:?}"
    );
}

struct FailedWriteClipboard(IdentifiedClipboard);

impl HostClipboard for FailedWriteClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.0.read()
    }
    fn read_with_identity(
        &self,
    ) -> Result<
        (
            HostClipboardSnapshot,
            Option<uc_engine::HostClipboardIdentity>,
        ),
        HostCapabilityError,
    > {
        self.0.read_with_identity()
    }
    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.write_with_receipt(snapshot).map(|_| ())
    }
    fn write_with_receipt(
        &self,
        snapshot: HostClipboardSnapshot,
    ) -> Result<uc_engine::HostClipboardWriteReceipt, HostCapabilityError> {
        let _ = self.0.write_with_receipt(snapshot)?;
        Err(HostCapabilityError::new(
            uc_engine::HostCapabilityErrorCategory::Io,
            "synthetic write failure",
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_partial_write_does_not_retire_later_user_copy() {
    let root = tempfile::tempdir().unwrap();
    let clipboard = IdentifiedClipboard::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(FailedWriteClipboard(clipboard.clone())),
        Box::new(EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(entry) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "synthetic restored text".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored entry")
    };
    assert!(engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: entry.entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .is_err());
    clipboard.copy_text("synthetic different user text");
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let history = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(history, OperationResult::HistoryStats(ref stats) if stats.total_items == 2),
        "{history:?}"
    );
}

#[derive(Clone, Default)]
struct CapturedClipboard {
    inner: IdentifiedClipboard,
    writes: Arc<
        Mutex<
            Vec<(
                HostClipboardSnapshot,
                Option<uc_engine::HostClipboardIdentity>,
            )>,
        >,
    >,
    staged: Arc<
        Mutex<
            Option<(
                HostClipboardSnapshot,
                Option<uc_engine::HostClipboardIdentity>,
            )>,
        >,
    >,
}

impl HostClipboard for CapturedClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.read_with_identity().map(|(snapshot, _)| snapshot)
    }
    fn read_with_identity(
        &self,
    ) -> Result<
        (
            HostClipboardSnapshot,
            Option<uc_engine::HostClipboardIdentity>,
        ),
        HostCapabilityError,
    > {
        if let Some(captured) = self.staged.lock().unwrap().take() {
            return Ok(captured);
        }
        self.inner.read_with_identity()
    }
    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.write_with_receipt(snapshot).map(|_| ())
    }
    fn write_with_receipt(
        &self,
        snapshot: HostClipboardSnapshot,
    ) -> Result<uc_engine::HostClipboardWriteReceipt, HostCapabilityError> {
        let receipt = self.inner.write_with_receipt(snapshot)?;
        self.writes
            .lock()
            .unwrap()
            .push(self.inner.read_with_identity()?);
        Ok(receipt)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_writes_and_delayed_captured_observations_keep_their_identity() {
    let root = tempfile::tempdir().unwrap();
    let clipboard = CapturedClipboard::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(clipboard.clone()),
        Box::new(EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let mut ids = Vec::new();
    for text in ["synthetic first text", "synthetic second text"] {
        let OperationResult::EntrySent(entry) = engine
            .execute(Operation::SendText(SendTextInput {
                text: text.into(),
                target_devices: Vec::new(),
            }))
            .await
            .unwrap()
        else {
            panic!("expected stored entry")
        };
        ids.push(entry.entry_id);
    }
    let first = engine.execute(Operation::RestoreClipboard(RestoreClipboardInput {
        entry_id: ids[0].clone(),
        mode: ClipboardRestoreMode::Standard,
    }));
    let second = engine.execute(Operation::RestoreClipboard(RestoreClipboardInput {
        entry_id: ids[1].clone(),
        mode: ClipboardRestoreMode::Standard,
    }));
    let (first, second) = tokio::join!(first, second);
    first.unwrap();
    second.unwrap();
    let writes = clipboard.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 2);
    for captured in [writes[1].clone(), writes[0].clone(), writes[0].clone()] {
        *clipboard.staged.lock().unwrap() = Some(captured);
        engine
            .execute(Operation::ObserveClipboardChange(
                ObserveClipboardChangeInput { dispatch: false },
            ))
            .await
            .unwrap();
    }
    let after_echo = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    clipboard.inner.copy_text("synthetic next user text");
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let after_copy = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(after_echo, OperationResult::HistoryStats(ref stats) if stats.total_items == 2),
        "{after_echo:?}"
    );
    assert!(
        matches!(after_copy, OperationResult::HistoryStats(ref stats) if stats.total_items == 3),
        "{after_copy:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identified_large_image_projection_preserves_next_copy() {
    let root = tempfile::tempdir().unwrap();
    let clipboard = IdentifiedClipboard::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(clipboard.clone()),
        Box::new(EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated host".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    let bytes =
        include_bytes!("../../../../tests/hosts/connectivity/tests/fixtures/active-clipboard.png")
            .to_vec();
    assert!(bytes.len() > 64 * 1024);
    let OperationResult::EntrySent(entry) = engine
        .execute(Operation::SendImage(uc_engine::SendImageInput {
            bytes,
            mime_type: "image/png".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored image")
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let result = engine
                .execute(Operation::GetHistoryEntryResource(
                    uc_engine::HistoryEntryInput {
                        entry_id: entry.entry_id.clone(),
                    },
                ))
                .await
                .unwrap();
            if matches!(
                result,
                OperationResult::HistoryEntryResource(uc_engine::HistoryEntryResourceSummary {
                    blob_id: Some(_),
                    inline_data: None,
                    ..
                })
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: entry.entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let after_echo = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    clipboard.copy_text("synthetic next user text");
    engine
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: false },
        ))
        .await
        .unwrap();
    let after_copy = engine.execute(Operation::QueryHistoryStats).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert!(
        matches!(after_echo, OperationResult::HistoryStats(ref stats) if stats.total_items == 1),
        "{after_echo:?}"
    );
    assert!(
        matches!(after_copy, OperationResult::HistoryStats(ref stats) if stats.total_items == 2),
        "{after_copy:?}"
    );
}
