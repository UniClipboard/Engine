use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::Mutex;
use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
use uc_core::{
    FileTransferCancellationReason, FileTransferDirection, FileTransferEvent,
    FileTransferFailureReason, FileTransferProgress,
};

use crate::file_transfer::FileTransferApplicationError;

use super::BeginReceiverTransfer;

#[derive(Default)]
pub(crate) struct FileTransferSessionRegistry {
    create_gate: Mutex<()>,
    sessions: Mutex<HashMap<String, Arc<FileTransferSession>>>,
    closed: Mutex<bool>,
}

impl FileTransferSessionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn lock_creation(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.create_gate.lock().await
    }

    pub(crate) async fn ensure_open(&self) -> Result<(), FileTransferApplicationError> {
        if *self.closed.lock().await {
            Err(FileTransferApplicationError::LifecycleClosed)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn get(&self, transfer_id: &str) -> Option<Arc<FileTransferSession>> {
        self.sessions.lock().await.get(transfer_id).cloned()
    }

    pub(crate) async fn insert(&self, session: Arc<FileTransferSession>) {
        self.sessions
            .lock()
            .await
            .insert(session.transfer_id().to_owned(), session);
    }

    pub(crate) async fn remove(&self, transfer_id: &str, expected: &FileTransferSession) {
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(transfer_id)
            .is_some_and(|registered| std::ptr::eq(Arc::as_ptr(registered), expected))
        {
            sessions.remove(transfer_id);
        }
    }

    pub(crate) async fn close_and_snapshot(&self) -> Vec<Arc<FileTransferSession>> {
        *self.closed.lock().await = true;
        self.sessions.lock().await.values().cloned().collect()
    }

    pub(crate) async fn snapshot(&self) -> Vec<Arc<FileTransferSession>> {
        self.sessions.lock().await.values().cloned().collect()
    }
}

#[derive(Default)]
struct SessionState {
    last_progress_bytes: Option<u64>,
    terminal_event: Option<FileTransferEvent>,
}

pub struct FileTransferSession {
    descriptor: BeginReceiverTransfer,
    store: Arc<dyn FileTransferEventStorePort>,
    publisher: Arc<dyn FileTransferEventPublisherPort>,
    registry: Weak<FileTransferSessionRegistry>,
    state: Mutex<SessionState>,
}

impl std::fmt::Debug for FileTransferSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileTransferSession")
            .field("transfer_id", &self.transfer_id())
            .finish_non_exhaustive()
    }
}

impl FileTransferSession {
    pub(crate) fn new(
        descriptor: BeginReceiverTransfer,
        store: Arc<dyn FileTransferEventStorePort>,
        publisher: Arc<dyn FileTransferEventPublisherPort>,
        registry: Weak<FileTransferSessionRegistry>,
        last_progress_bytes: Option<u64>,
    ) -> Self {
        Self {
            descriptor,
            store,
            publisher,
            registry,
            state: Mutex::new(SessionState {
                last_progress_bytes,
                terminal_event: None,
            }),
        }
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
            .map_err(|error| FileTransferApplicationError::Store(error.to_string()))?;
        state.last_progress_bytes = Some(bytes_transferred);
        self.publisher
            .publish(event.clone())
            .await
            .map_err(|error| FileTransferApplicationError::Publish(error.to_string()))?;
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
        if let Some(existing) = state.terminal_event.as_ref() {
            return if existing == &event {
                Ok(existing.clone())
            } else {
                Err(FileTransferApplicationError::TransferAlreadyFinished {
                    transfer_id: self.transfer_id().to_owned(),
                })
            };
        }

        self.store
            .append(event.clone())
            .await
            .map_err(|error| FileTransferApplicationError::Store(error.to_string()))?;
        state.terminal_event = Some(event.clone());
        let publish_result = self
            .publisher
            .publish(event.clone())
            .await
            .map_err(|error| FileTransferApplicationError::Publish(error.to_string()));
        drop(state);

        if let Some(registry) = self.registry.upgrade() {
            registry.remove(self.transfer_id(), self).await;
        }
        publish_result?;
        Ok(event)
    }

    fn ensure_active(&self, state: &SessionState) -> Result<(), FileTransferApplicationError> {
        if state.terminal_event.is_some() {
            Err(FileTransferApplicationError::TransferAlreadyFinished {
                transfer_id: self.transfer_id().to_owned(),
            })
        } else {
            Ok(())
        }
    }
}
