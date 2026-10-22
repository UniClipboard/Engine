use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use tokio::sync::Mutex;
use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
use uc_core::{
    FileTransferCancellationReason, FileTransferDirection, FileTransferEvent,
    FileTransferFailureReason, FileTransferProgress,
};

use crate::transfer::file::FileTransferApplicationError;

use crate::transfer::file::facade::BeginReceiverTransfer;

#[derive(Default)]
pub(crate) struct ReceiverTransferHandleRegistry {
    create_gate: Mutex<()>,
    sessions: Mutex<HashMap<String, Arc<ReceiverTransferHandle>>>,
    closed: AtomicBool,
}

impl ReceiverTransferHandleRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn lock_creation(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.create_gate.lock().await
    }

    pub(crate) async fn ensure_open(&self) -> Result<(), FileTransferApplicationError> {
        if self.closed.load(Ordering::Acquire) {
            Err(FileTransferApplicationError::LifecycleClosed)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn get(&self, transfer_id: &str) -> Option<Arc<ReceiverTransferHandle>> {
        self.sessions.lock().await.get(transfer_id).cloned()
    }

    pub(crate) async fn insert(&self, session: Arc<ReceiverTransferHandle>) {
        self.sessions
            .lock()
            .await
            .insert(session.transfer_id().to_owned(), session);
    }

    pub(crate) async fn remove(&self, transfer_id: &str, expected: &ReceiverTransferHandle) {
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(transfer_id)
            .is_some_and(|registered| std::ptr::eq(Arc::as_ptr(registered), expected))
        {
            sessions.remove(transfer_id);
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) async fn snapshot(&self) -> Vec<Arc<ReceiverTransferHandle>> {
        self.sessions.lock().await.values().cloned().collect()
    }
}

#[derive(Default)]
struct SessionState {
    last_progress_bytes: Option<u64>,
    started_stored: bool,
    started_published: bool,
    terminal_intent: Option<FileTransferEvent>,
    terminal_stored: bool,
    terminal_published: bool,
}

pub struct ReceiverTransferHandle {
    descriptor: BeginReceiverTransfer,
    store: Arc<dyn FileTransferEventStorePort>,
    publisher: Arc<dyn FileTransferEventPublisherPort>,
    registry: Weak<ReceiverTransferHandleRegistry>,
    state: Mutex<SessionState>,
}

impl std::fmt::Debug for ReceiverTransferHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReceiverTransferHandle")
            .field("transfer_id", &self.transfer_id())
            .finish_non_exhaustive()
    }
}

impl ReceiverTransferHandle {
    pub(crate) fn new(
        descriptor: BeginReceiverTransfer,
        store: Arc<dyn FileTransferEventStorePort>,
        publisher: Arc<dyn FileTransferEventPublisherPort>,
        registry: Weak<ReceiverTransferHandleRegistry>,
        last_progress_bytes: Option<u64>,
        started: bool,
    ) -> Self {
        Self {
            descriptor,
            store,
            publisher,
            registry,
            state: Mutex::new(SessionState {
                last_progress_bytes,
                started_stored: started,
                started_published: started,
                terminal_intent: None,
                terminal_stored: false,
                terminal_published: false,
            }),
        }
    }

    pub(crate) async fn start(&self) -> Result<(), FileTransferApplicationError> {
        let mut state = self.state.lock().await;
        self.ensure_active(&state)?;
        if let Err(error) = self.store_started(&mut state).await {
            // 准备失败已确定；关闭不能把同一原任务的失败改写成取消。
            state.terminal_intent = Some(FileTransferEvent::failed(
                self.transfer_id(),
                &self.descriptor.peer_id,
                FileTransferFailureReason::StorageUnavailable,
                None,
            ));
            return Err(error);
        }
        Ok(())
    }

    async fn store_started(
        &self,
        state: &mut SessionState,
    ) -> Result<(), FileTransferApplicationError> {
        let event = FileTransferEvent::started(
            self.transfer_id(),
            &self.descriptor.peer_id,
            &self.descriptor.filename,
            self.descriptor.file_size,
        );
        if !state.started_stored {
            self.store
                .append(event.clone())
                .await
                .map_err(FileTransferApplicationError::Store)?;
            state.started_stored = true;
        }
        if !state.started_published {
            self.publisher
                .publish(event)
                .await
                .map_err(FileTransferApplicationError::Publish)?;
            state.started_published = true;
        }
        Ok(())
    }

    pub fn transfer_id(&self) -> &str {
        &self.descriptor.transfer_id
    }

    pub(crate) fn matches(&self, descriptor: &BeginReceiverTransfer) -> bool {
        self.descriptor == *descriptor
    }

    pub async fn bytes_transferred(&self) -> u64 {
        self.state.lock().await.last_progress_bytes.unwrap_or(0)
    }

    pub async fn report_progress(
        &self,
        bytes_transferred: u64,
        total_bytes: Option<u64>,
    ) -> Result<FileTransferEvent, FileTransferApplicationError> {
        let mut state = self.state.lock().await;
        self.ensure_active(&state)?;
        if let Some(previous_bytes) = state.last_progress_bytes {
            if bytes_transferred < previous_bytes {
                return Err(FileTransferApplicationError::ProgressWentBackwards {
                    transfer_id: self.transfer_id().to_owned(),
                    previous_bytes,
                    new_bytes: bytes_transferred,
                });
            }
        }

        let event = FileTransferEvent::Progress {
            transfer_id: self.transfer_id().to_owned(),
            peer_id: self.descriptor.peer_id.clone(),
            progress: FileTransferProgress {
                direction: FileTransferDirection::Receiving,
                bytes_transferred,
                total_bytes,
            },
        };
        self.store
            .append(event.clone())
            .await
            .map_err(FileTransferApplicationError::Store)?;
        state.last_progress_bytes = Some(bytes_transferred);
        self.publisher
            .publish(event.clone())
            .await
            .map_err(FileTransferApplicationError::Publish)?;
        Ok(event)
    }

    pub async fn complete(&self) -> Result<FileTransferEvent, FileTransferApplicationError> {
        self.finish(FileTransferEvent::completed(
            self.transfer_id(),
            &self.descriptor.peer_id,
        ))
        .await
    }

    pub async fn fail(
        &self,
        reason: FileTransferFailureReason,
        detail: Option<String>,
    ) -> Result<FileTransferEvent, FileTransferApplicationError> {
        self.finish(FileTransferEvent::failed(
            self.transfer_id(),
            &self.descriptor.peer_id,
            reason,
            detail,
        ))
        .await
    }

    pub async fn cancel(
        &self,
        reason: FileTransferCancellationReason,
    ) -> Result<FileTransferEvent, FileTransferApplicationError> {
        self.finish(FileTransferEvent::cancelled(
            self.transfer_id(),
            &self.descriptor.peer_id,
            reason,
        ))
        .await
    }

    async fn finish(
        &self,
        event: FileTransferEvent,
    ) -> Result<FileTransferEvent, FileTransferApplicationError> {
        let mut state = self.state.lock().await;
        if let Some(existing) = state.terminal_intent.as_ref() {
            if existing != &event {
                return Err(FileTransferApplicationError::TransferAlreadyFinished {
                    transfer_id: self.transfer_id().to_owned(),
                });
            }
        } else {
            // 意图先封住后续进度和不同终态；失败时仍由同一原任务恢复该意图。
            state.terminal_intent = Some(event.clone());
        }
        self.store_started(&mut state).await?;
        if !state.terminal_stored {
            self.store
                .append(event.clone())
                .await
                .map_err(FileTransferApplicationError::Store)?;
            state.terminal_stored = true;
        }
        if !state.terminal_published {
            self.publisher
                .publish(event.clone())
                .await
                .map_err(FileTransferApplicationError::Publish)?;
            state.terminal_published = true;
        }
        drop(state);

        if let Some(registry) = self.registry.upgrade() {
            registry.remove(self.transfer_id(), self).await;
        }
        Ok(event)
    }

    fn ensure_active(&self, state: &SessionState) -> Result<(), FileTransferApplicationError> {
        if state.terminal_intent.is_some() {
            Err(FileTransferApplicationError::TransferAlreadyFinished {
                transfer_id: self.transfer_id().to_owned(),
            })
        } else {
            Ok(())
        }
    }
}
