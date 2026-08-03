use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;
use uc_core::mobile_sync::{MobileDeviceId, StagedFile, StagingHandle};
use uc_core::ports::MobileFileStagingPort;
use uc_core::{FileTransferCancellationReason, FileTransferFailureReason};

use crate::facade::file_transfer::{
    BeginReceiverTransfer, FileTransferFacade, FileTransferSession, ReceiverTransferRegistration,
};
use crate::usecases::mobile_sync::apply_incoming::{
    ApplyIncomingMobileClipError, ApplyIncomingMobileClipInput, ApplyIncomingMobileClipOutcome,
    ApplyIncomingMobileClipUseCase, IncomingMobileClipEvent,
};

const MOBILE_UPLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const MOBILE_UPLOAD_HANDLE_PREFIX: &str = "uc-mobile-upload-v1:";

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MobileFileUploadHandle(String);

impl MobileFileUploadHandle {
    fn new() -> Self {
        Self(format!(
            "{MOBILE_UPLOAD_HANDLE_PREFIX}{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for MobileFileUploadHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MobileFileUploadHandle([REDACTED])")
    }
}

#[derive(Debug, Clone)]
pub struct BeginMobileFileUpload {
    pub data_name: String,
    pub media_type: String,
    pub source_device_id: MobileDeviceId,
    pub transfer_id: String,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MobileFileUploadError {
    #[error("mobile file upload input is invalid")]
    InvalidInput,
    #[error("mobile file upload handle is unknown")]
    UnknownHandle,
    #[error("mobile file upload coordinator is closed")]
    Closed,
    #[error("mobile file upload is unavailable")]
    Unavailable,
    #[error("mobile file upload failed")]
    UploadFailed,
    #[error("mobile file upload completion failed")]
    CompletionFailed(#[source] ApplyIncomingMobileClipError),
}

pub(crate) struct CompleteMobileFileUpload {
    pub(crate) data_name: String,
    pub(crate) media_type: String,
    pub(crate) source_device_id: MobileDeviceId,
    pub(crate) transfer_id: String,
    pub(crate) staged: StagedFile,
}

#[async_trait]
pub(crate) trait MobileFileUploadApplyPort: Send + Sync {
    async fn complete_mobile_file_upload(
        &self,
        input: CompleteMobileFileUpload,
    ) -> Result<ApplyIncomingMobileClipOutcome, ApplyIncomingMobileClipError>;
}

#[async_trait]
impl MobileFileUploadApplyPort for ApplyIncomingMobileClipUseCase {
    async fn complete_mobile_file_upload(
        &self,
        input: CompleteMobileFileUpload,
    ) -> Result<ApplyIncomingMobileClipOutcome, ApplyIncomingMobileClipError> {
        self.execute(ApplyIncomingMobileClipInput {
            source_device_id: input.source_device_id,
            event: IncomingMobileClipEvent::BufferFile {
                data_name: input.data_name,
                mime: input.media_type,
                staged: input.staged,
                transfer_id: input.transfer_id,
            },
        })
        .await
    }
}

struct ActiveMobileFileUpload {
    staging: StagingHandle,
    data_name: String,
    source_device_id: MobileDeviceId,
    transfer_id: String,
    total_bytes: Option<u64>,
    bytes_received: u64,
    last_progress_at: Instant,
    session: Arc<FileTransferSession>,
}

type ActiveMobileFileUploadState = Arc<Mutex<Option<ActiveMobileFileUpload>>>;

#[derive(Default)]
struct UploadRegistry {
    closed: bool,
    uploads: HashMap<String, ActiveMobileFileUploadState>,
}

pub(crate) struct MobileFileUploadCoordinator {
    staging: Arc<dyn MobileFileStagingPort>,
    apply: Arc<dyn MobileFileUploadApplyPort>,
    file_transfer: Option<Arc<FileTransferFacade>>,
    progress_interval: Duration,
    lifecycle_gate: RwLock<()>,
    registry: Mutex<UploadRegistry>,
}

impl MobileFileUploadCoordinator {
    pub(crate) fn new(
        staging: Arc<dyn MobileFileStagingPort>,
        apply: Arc<dyn MobileFileUploadApplyPort>,
        file_transfer: Option<Arc<FileTransferFacade>>,
    ) -> Self {
        Self::with_progress_interval(
            staging,
            apply,
            file_transfer,
            MOBILE_UPLOAD_PROGRESS_INTERVAL,
        )
    }

    fn with_progress_interval(
        staging: Arc<dyn MobileFileStagingPort>,
        apply: Arc<dyn MobileFileUploadApplyPort>,
        file_transfer: Option<Arc<FileTransferFacade>>,
        progress_interval: Duration,
    ) -> Self {
        Self {
            staging,
            apply,
            file_transfer,
            progress_interval,
            lifecycle_gate: RwLock::new(()),
            registry: Mutex::new(UploadRegistry::default()),
        }
    }

    pub(crate) async fn begin_upload(
        &self,
        input: BeginMobileFileUpload,
    ) -> Result<MobileFileUploadHandle, MobileFileUploadError> {
        if input.data_name.trim().is_empty()
            || input.media_type.trim().is_empty()
            || input.source_device_id.as_str().trim().is_empty()
            || input.transfer_id.trim().is_empty()
        {
            return Err(MobileFileUploadError::InvalidInput);
        }

        let _operation = self.lifecycle_gate.read().await;
        if self.registry.lock().await.closed {
            return Err(MobileFileUploadError::Closed);
        }

        let peer_id = format!("mobile:{}", input.source_device_id.as_str());
        let file_transfer = self
            .file_transfer
            .as_ref()
            .ok_or(MobileFileUploadError::Unavailable)?;
        let session = file_transfer
            .begin_receiver_transfer(BeginReceiverTransfer {
                transfer_id: input.transfer_id.clone(),
                peer_id,
                filename: input.data_name.clone(),
                file_size: input.total_bytes,
                registration: ReceiverTransferRegistration::Provisional,
            })
            .await
            .map_err(|_| MobileFileUploadError::UploadFailed)?;
        if session.report_progress(0, input.total_bytes).await.is_err() {
            Self::fail_session(&session, "mobile upload progress failed").await;
            return Err(MobileFileUploadError::UploadFailed);
        }

        let scope_id = streaming_scope_nonce();
        let staging = match self
            .staging
            .begin_stage(&scope_id, &input.data_name, &input.media_type)
            .await
        {
            Ok(staging) => staging,
            Err(_) => {
                Self::fail_session(&session, "mobile upload staging failed").await;
                return Err(MobileFileUploadError::UploadFailed);
            }
        };

        let upload = Arc::new(Mutex::new(Some(ActiveMobileFileUpload {
            staging,
            data_name: input.data_name,
            source_device_id: input.source_device_id,
            transfer_id: input.transfer_id,
            total_bytes: input.total_bytes,
            bytes_received: 0,
            last_progress_at: Instant::now(),
            session,
        })));
        let mut registry = self.registry.lock().await;
        loop {
            let handle = MobileFileUploadHandle::new();
            if !registry.uploads.contains_key(handle.as_str()) {
                registry
                    .uploads
                    .insert(handle.as_str().to_owned(), Arc::clone(&upload));
                return Ok(handle);
            }
        }
    }

    pub(crate) async fn append_chunk(
        &self,
        handle: &MobileFileUploadHandle,
        chunk: &[u8],
    ) -> Result<(), MobileFileUploadError> {
        let _operation = self.lifecycle_gate.read().await;
        let appended_bytes =
            u64::try_from(chunk.len()).map_err(|_| MobileFileUploadError::InvalidInput)?;
        let upload = self
            .registry
            .lock()
            .await
            .uploads
            .get(handle.as_str())
            .cloned()
            .ok_or(MobileFileUploadError::UnknownHandle)?;
        let mut state = upload.lock().await;
        let staging = state
            .as_ref()
            .map(|active| active.staging.clone())
            .ok_or(MobileFileUploadError::UnknownHandle)?;

        if self
            .staging
            .append_stage_chunk(&staging, chunk)
            .await
            .is_err()
        {
            let failed = state.take();
            drop(state);
            self.remove_if_registered(handle, &upload).await;
            if let Some(failed) = failed {
                self.cleanup_failed_upload(failed, "mobile upload append failed")
                    .await;
            }
            return Err(MobileFileUploadError::UploadFailed);
        }

        let progress = state.as_mut().and_then(|active| {
            active.bytes_received = active.bytes_received.saturating_add(appended_bytes);
            (active.last_progress_at.elapsed() >= self.progress_interval).then(|| {
                (
                    Arc::clone(&active.session),
                    active.bytes_received,
                    active.total_bytes,
                )
            })
        });
        if let Some((session, bytes_received, total_bytes)) = progress {
            if session
                .report_progress(bytes_received, total_bytes)
                .await
                .is_err()
            {
                let failed = state.take();
                drop(state);
                self.remove_if_registered(handle, &upload).await;
                if let Some(failed) = failed {
                    self.cleanup_failed_upload(failed, "mobile upload progress failed")
                        .await;
                }
                return Err(MobileFileUploadError::UploadFailed);
            }
            if let Some(active) = state.as_mut() {
                active.last_progress_at = Instant::now();
            }
        }
        Ok(())
    }

    pub(crate) async fn finish_upload(
        &self,
        handle: MobileFileUploadHandle,
        media_type: String,
    ) -> Result<ApplyIncomingMobileClipOutcome, MobileFileUploadError> {
        let _operation = self.lifecycle_gate.read().await;
        if media_type.trim().is_empty() {
            return Err(MobileFileUploadError::InvalidInput);
        }
        let upload = self.take_registered(&handle).await?;
        if upload
            .session
            .report_progress(
                upload.bytes_received,
                upload.total_bytes.or(Some(upload.bytes_received)),
            )
            .await
            .is_err()
        {
            self.cleanup_failed_upload(upload, "mobile upload progress failed")
                .await;
            return Err(MobileFileUploadError::UploadFailed);
        }

        let staged = match self.staging.finalize_stage(upload.staging).await {
            Ok(staged) => staged,
            Err(_) => {
                Self::fail_session(&upload.session, "mobile upload finalize failed").await;
                return Err(MobileFileUploadError::CompletionFailed(
                    ApplyIncomingMobileClipError::Internal(
                        "mobile file staging finalization failed".into(),
                    ),
                ));
            }
        };
        let result = self
            .apply
            .complete_mobile_file_upload(CompleteMobileFileUpload {
                data_name: upload.data_name,
                media_type,
                source_device_id: upload.source_device_id,
                transfer_id: upload.transfer_id,
                staged,
            })
            .await;
        match result {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                Self::fail_session(&upload.session, "mobile upload apply failed").await;
                Err(MobileFileUploadError::CompletionFailed(error))
            }
        }
    }

    pub(crate) async fn abort_upload(
        &self,
        handle: MobileFileUploadHandle,
    ) -> Result<bool, MobileFileUploadError> {
        let _operation = self.lifecycle_gate.read().await;
        let Some(upload) = self.take_registered_if_present(&handle).await else {
            return Ok(false);
        };
        self.staging.abort_stage(upload.staging).await;
        upload
            .session
            .cancel(FileTransferCancellationReason::LocalUser)
            .await
            .map_err(|_| MobileFileUploadError::UploadFailed)?;
        Ok(true)
    }

    pub(crate) async fn close(&self) -> Result<(), MobileFileUploadError> {
        let _lifecycle = self.lifecycle_gate.write().await;
        let uploads = {
            let mut registry = self.registry.lock().await;
            registry.closed = true;
            std::mem::take(&mut registry.uploads)
        };
        let mut failed = false;
        for upload in uploads.into_values() {
            let Some(upload) = upload.lock().await.take() else {
                continue;
            };
            self.staging.abort_stage(upload.staging).await;
            if upload
                .session
                .cancel(FileTransferCancellationReason::Unknown)
                .await
                .is_err()
            {
                failed = true;
                warn!("mobile file upload close could not settle one transfer");
            }
        }
        if failed {
            Err(MobileFileUploadError::UploadFailed)
        } else {
            Ok(())
        }
    }

    async fn take_registered(
        &self,
        handle: &MobileFileUploadHandle,
    ) -> Result<ActiveMobileFileUpload, MobileFileUploadError> {
        self.take_registered_if_present(handle)
            .await
            .ok_or(MobileFileUploadError::UnknownHandle)
    }

    async fn take_registered_if_present(
        &self,
        handle: &MobileFileUploadHandle,
    ) -> Option<ActiveMobileFileUpload> {
        let upload = self.registry.lock().await.uploads.remove(handle.as_str())?;
        let active = upload.lock().await.take();
        active
    }

    async fn remove_if_registered(
        &self,
        handle: &MobileFileUploadHandle,
        expected: &ActiveMobileFileUploadState,
    ) {
        let mut registry = self.registry.lock().await;
        if registry
            .uploads
            .get(handle.as_str())
            .is_some_and(|registered| Arc::ptr_eq(registered, expected))
        {
            registry.uploads.remove(handle.as_str());
        }
    }

    async fn cleanup_failed_upload(&self, upload: ActiveMobileFileUpload, detail: &'static str) {
        self.staging.abort_stage(upload.staging).await;
        Self::fail_session(&upload.session, detail).await;
    }

    async fn fail_session(session: &FileTransferSession, detail: &'static str) {
        if session
            .fail(FileTransferFailureReason::Unknown, Some(detail.to_owned()))
            .await
            .is_err()
        {
            warn!("mobile file upload failure could not settle transfer");
        }
    }
}

fn streaming_scope_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::Notify;
    use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
    use uc_core::mobile_sync::{MobileDeviceId, StagedFile, StagedFileUri, StagingHandle};
    use uc_core::ports::file_transfer::{
        FileTransferProjectionError, PendingInboundTransfer, ProvisionalInboundTransfer,
        UpdateProvisionalReceivePathPort,
    };
    use uc_core::ports::{
        ClockPort, FinalizeProvisionalReceivePort, MobileFileStagingError, MobileFileStagingPort,
        ProvisionalReceiveAction, ProvisionalReceiveError, RecordReceiverTransferPort,
        SeedProvisionalReceivePort,
    };
    use uc_core::{FileTransferCancellationReason, FileTransferEvent, FileTransferFailureReason};
    use uc_infra::file_transfer::{InMemoryEventPublisher, InMemoryEventStore};

    use crate::facade::file_transfer::{FileTransferFacade, FileTransferFacadeDeps};
    use crate::usecases::mobile_sync::apply_incoming::{
        ApplyIncomingMobileClipError, ApplyIncomingMobileClipOutcome,
    };

    use super::{
        BeginMobileFileUpload, CompleteMobileFileUpload, MobileFileUploadApplyPort,
        MobileFileUploadCoordinator, MobileFileUploadError, MobileFileUploadHandle,
    };

    #[derive(Default)]
    struct ReceiverStore;

    #[async_trait]
    impl RecordReceiverTransferPort for ReceiverStore {
        async fn upsert_pending_transfer(
            &self,
            _transfer: &PendingInboundTransfer,
        ) -> Result<(), FileTransferProjectionError> {
            Ok(())
        }
    }

    #[async_trait]
    impl SeedProvisionalReceivePort for ReceiverStore {
        async fn seed_provisional_receive(
            &self,
            _transfer: &ProvisionalInboundTransfer,
        ) -> Result<(), ProvisionalReceiveError> {
            Ok(())
        }
    }

    #[async_trait]
    impl UpdateProvisionalReceivePathPort for ReceiverStore {
        async fn update_provisional_receive_path(
            &self,
            _provisional_transfer_id: &str,
            _cached_path: &str,
            _now_ms: i64,
        ) -> Result<(), ProvisionalReceiveError> {
            Ok(())
        }
    }

    #[async_trait]
    impl FinalizeProvisionalReceivePort for ReceiverStore {
        async fn finalize_provisional_receive(
            &self,
            _provisional_transfer_id: &str,
            _action: ProvisionalReceiveAction,
            _now_ms: i64,
        ) -> Result<(), ProvisionalReceiveError> {
            Ok(())
        }
    }

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            42
        }
    }

    #[derive(Default)]
    struct FakeStaging {
        active: Mutex<HashMap<uuid::Uuid, Vec<u8>>>,
        aborted: AtomicUsize,
        append_calls: AtomicUsize,
        concurrent_appends: AtomicUsize,
        max_concurrent_appends: AtomicUsize,
        fail_begin: AtomicBool,
        fail_append: AtomicBool,
        fail_finalize: AtomicBool,
        block_append: AtomicBool,
        append_started: Notify,
        append_release: Notify,
    }

    impl FakeStaging {
        fn abort_count(&self) -> usize {
            self.aborted.load(Ordering::SeqCst)
        }

        fn max_concurrent_appends(&self) -> usize {
            self.max_concurrent_appends.load(Ordering::SeqCst)
        }

        fn active_count(&self) -> usize {
            self.active.lock().map(|active| active.len()).unwrap_or(0)
        }
    }

    #[async_trait]
    impl MobileFileStagingPort for FakeStaging {
        async fn stage_file(
            &self,
            _scope_id: &str,
            _data_name: &str,
            _mime: &str,
            _bytes: Vec<u8>,
        ) -> Result<StagedFile, MobileFileStagingError> {
            Err(MobileFileStagingError::Io("unused".into()))
        }

        async fn read_by_uri(&self, _uri: &str) -> Result<Vec<u8>, MobileFileStagingError> {
            Err(MobileFileStagingError::NotFound)
        }

        async fn begin_stage(
            &self,
            _scope_id: &str,
            _data_name: &str,
            _mime: &str,
        ) -> Result<StagingHandle, MobileFileStagingError> {
            if self.fail_begin.load(Ordering::SeqCst) {
                return Err(MobileFileStagingError::Io("begin failed".into()));
            }
            let handle = StagingHandle::new();
            self.active
                .lock()
                .map_err(|_| MobileFileStagingError::Io("active lock poisoned".into()))?
                .insert(handle.token(), Vec::new());
            Ok(handle)
        }

        async fn append_stage_chunk(
            &self,
            handle: &StagingHandle,
            chunk: &[u8],
        ) -> Result<(), MobileFileStagingError> {
            self.append_calls.fetch_add(1, Ordering::SeqCst);
            let concurrent = self.concurrent_appends.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_concurrent_appends
                .fetch_max(concurrent, Ordering::SeqCst);
            if self.block_append.load(Ordering::SeqCst) {
                self.append_started.notify_one();
                self.append_release.notified().await;
            }
            tokio::task::yield_now().await;

            let result = if self.fail_append.load(Ordering::SeqCst) {
                Err(MobileFileStagingError::Io("append failed".into()))
            } else {
                self.active
                    .lock()
                    .map_err(|_| MobileFileStagingError::Io("active lock poisoned".into()))?
                    .get_mut(&handle.token())
                    .ok_or_else(|| MobileFileStagingError::Io("unknown staging handle".into()))?
                    .extend_from_slice(chunk);
                Ok(())
            };
            self.concurrent_appends.fetch_sub(1, Ordering::SeqCst);
            result
        }

        async fn finalize_stage(
            &self,
            handle: StagingHandle,
        ) -> Result<StagedFile, MobileFileStagingError> {
            let bytes = self
                .active
                .lock()
                .map_err(|_| MobileFileStagingError::Io("active lock poisoned".into()))?
                .remove(&handle.token())
                .ok_or_else(|| MobileFileStagingError::Io("unknown staging handle".into()))?;
            if self.fail_finalize.load(Ordering::SeqCst) {
                return Err(MobileFileStagingError::Io("finalize failed".into()));
            }
            Ok(StagedFile {
                uri: StagedFileUri::new(format!("file:///staged-{}.bin", bytes.len())),
                sanitized_name: "staged.bin".into(),
            })
        }

        async fn abort_stage(&self, handle: StagingHandle) {
            if self
                .active
                .lock()
                .ok()
                .and_then(|mut active| active.remove(&handle.token()))
                .is_some()
            {
                self.aborted.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[derive(Default)]
    struct FakeApply {
        completed: Mutex<Vec<CompleteMobileFileUpload>>,
    }

    #[async_trait]
    impl MobileFileUploadApplyPort for FakeApply {
        async fn complete_mobile_file_upload(
            &self,
            input: CompleteMobileFileUpload,
        ) -> Result<ApplyIncomingMobileClipOutcome, ApplyIncomingMobileClipError> {
            self.completed
                .lock()
                .map_err(|_| ApplyIncomingMobileClipError::Internal("apply lock poisoned".into()))?
                .push(input);
            Ok(ApplyIncomingMobileClipOutcome::Buffered)
        }
    }

    struct TestContext {
        coordinator: Arc<MobileFileUploadCoordinator>,
        staging: Arc<FakeStaging>,
        apply: Arc<FakeApply>,
        events: Arc<InMemoryEventStore>,
    }

    fn build_context() -> TestContext {
        let staging = Arc::new(FakeStaging::default());
        let apply = Arc::new(FakeApply::default());
        let events = Arc::new(InMemoryEventStore::new());
        let publisher = Arc::new(InMemoryEventPublisher::new());
        let receiver = Arc::new(ReceiverStore);
        let event_store: Arc<dyn FileTransferEventStorePort> = events.clone();
        let event_publisher: Arc<dyn FileTransferEventPublisherPort> = publisher;
        let repo: Arc<dyn RecordReceiverTransferPort> = receiver.clone();
        let provisional_seed: Arc<dyn SeedProvisionalReceivePort> = receiver.clone();
        let provisional_path: Arc<dyn UpdateProvisionalReceivePathPort> = receiver.clone();
        let provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort> = receiver;
        let transfers = Arc::new(FileTransferFacade::new(FileTransferFacadeDeps {
            store: event_store,
            publisher: event_publisher,
            repo,
            provisional_seed,
            provisional_path,
            provisional_finalize,
            clock: Arc::new(FixedClock),
        }));
        let staging_port: Arc<dyn MobileFileStagingPort> = staging.clone();
        let apply_port: Arc<dyn MobileFileUploadApplyPort> = apply.clone();
        TestContext {
            coordinator: Arc::new(MobileFileUploadCoordinator::with_progress_interval(
                staging_port,
                apply_port,
                Some(transfers),
                Duration::ZERO,
            )),
            staging,
            apply,
            events,
        }
    }

    fn begin_input(transfer_id: &str) -> BeginMobileFileUpload {
        BeginMobileFileUpload {
            data_name: "upload.bin".into(),
            media_type: "application/octet-stream".into(),
            source_device_id: MobileDeviceId::new("mobile-source"),
            transfer_id: transfer_id.into(),
            total_bytes: Some(6),
        }
    }

    async fn history(context: &TestContext, transfer_id: &str) -> Vec<FileTransferEvent> {
        context.events.load(transfer_id).await.unwrap()
    }

    #[tokio::test]
    async fn successful_finish_buffers_the_file_and_consumes_the_public_handle() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-success"))
            .await
            .unwrap();
        assert!(handle.as_str().starts_with("uc-mobile-upload-v1:"));

        context
            .coordinator
            .append_chunk(&handle, b"abcdef")
            .await
            .unwrap();
        assert_eq!(
            context
                .coordinator
                .finish_upload(handle.clone(), "application/octet-stream".into())
                .await
                .unwrap(),
            ApplyIncomingMobileClipOutcome::Buffered
        );

        assert!(matches!(
            context.coordinator.append_chunk(&handle, b"again").await,
            Err(MobileFileUploadError::UnknownHandle)
        ));
        let completed = context.apply.completed.lock().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].transfer_id, "transfer-success");
        assert_eq!(completed[0].staged.uri.as_str(), "file:///staged-6.bin");
        assert!(!history(&context, "transfer-success")
            .await
            .iter()
            .any(|event| {
                matches!(
                    event,
                    FileTransferEvent::Completed { .. }
                        | FileTransferEvent::Failed { .. }
                        | FileTransferEvent::Cancelled { .. }
                )
            }));
    }

    #[tokio::test]
    async fn append_failure_aborts_staging_marks_failed_and_invalidates_the_handle() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-append-failure"))
            .await
            .unwrap();
        context.staging.fail_append.store(true, Ordering::SeqCst);

        assert!(matches!(
            context.coordinator.append_chunk(&handle, b"broken").await,
            Err(MobileFileUploadError::UploadFailed)
        ));
        assert_eq!(context.staging.abort_count(), 1);
        assert_eq!(context.staging.active_count(), 0);
        assert!(matches!(
            context.coordinator.append_chunk(&handle, b"again").await,
            Err(MobileFileUploadError::UnknownHandle)
        ));
        assert!(matches!(
            history(&context, "transfer-append-failure").await.last(),
            Some(FileTransferEvent::Failed {
                reason: FileTransferFailureReason::Unknown,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn staging_begin_failure_marks_the_transfer_failed_without_creating_a_handle() {
        let context = build_context();
        context.staging.fail_begin.store(true, Ordering::SeqCst);

        assert!(matches!(
            context
                .coordinator
                .begin_upload(begin_input("transfer-begin-failure"))
                .await,
            Err(MobileFileUploadError::UploadFailed)
        ));
        assert_eq!(context.staging.active_count(), 0);
        assert!(matches!(
            history(&context, "transfer-begin-failure").await.last(),
            Some(FileTransferEvent::Failed {
                reason: FileTransferFailureReason::Unknown,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn staging_finish_failure_marks_failed_and_invalidates_the_handle() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-finish-failure"))
            .await
            .unwrap();
        context
            .coordinator
            .append_chunk(&handle, b"abcdef")
            .await
            .unwrap();
        context.staging.fail_finalize.store(true, Ordering::SeqCst);

        assert!(matches!(
            context
                .coordinator
                .finish_upload(handle.clone(), "application/octet-stream".into())
                .await,
            Err(MobileFileUploadError::CompletionFailed(_))
        ));
        assert_eq!(context.staging.active_count(), 0);
        assert!(matches!(
            context.coordinator.append_chunk(&handle, b"again").await,
            Err(MobileFileUploadError::UnknownHandle)
        ));
        assert!(matches!(
            history(&context, "transfer-finish-failure").await.last(),
            Some(FileTransferEvent::Failed {
                reason: FileTransferFailureReason::Unknown,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn concurrent_appends_are_serialized_by_the_coordinator() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-concurrent-append"))
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            context.coordinator.append_chunk(&handle, b"abc"),
            context.coordinator.append_chunk(&handle, b"def"),
        );

        first.unwrap();
        second.unwrap();
        assert_eq!(context.staging.max_concurrent_appends(), 1);
        context
            .coordinator
            .finish_upload(handle, "application/octet-stream".into())
            .await
            .unwrap();
        assert_eq!(
            context.apply.completed.lock().unwrap()[0]
                .staged
                .uri
                .as_str(),
            "file:///staged-6.bin"
        );
    }

    #[tokio::test]
    async fn concurrent_finishes_allow_only_one_consumer() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-concurrent-finish"))
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            context
                .coordinator
                .finish_upload(handle.clone(), "application/octet-stream".into()),
            context
                .coordinator
                .finish_upload(handle, "application/octet-stream".into()),
        );

        assert_ne!(first.is_ok(), second.is_ok());
        let error = first.err().or_else(|| second.err()).unwrap();
        assert!(matches!(error, MobileFileUploadError::UnknownHandle));
        assert_eq!(context.apply.completed.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn abort_is_idempotent_and_records_cancelled_terminal_state() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-abort"))
            .await
            .unwrap();

        assert!(context
            .coordinator
            .abort_upload(handle.clone())
            .await
            .unwrap());
        assert!(!context.coordinator.abort_upload(handle).await.unwrap());
        assert_eq!(context.staging.abort_count(), 1);
        assert!(matches!(
            history(&context, "transfer-abort").await.last(),
            Some(FileTransferEvent::Cancelled {
                reason: FileTransferCancellationReason::LocalUser,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn close_aborts_every_upload_and_rejects_new_ones() {
        let context = build_context();
        context
            .coordinator
            .begin_upload(begin_input("transfer-close-1"))
            .await
            .unwrap();
        context
            .coordinator
            .begin_upload(begin_input("transfer-close-2"))
            .await
            .unwrap();

        context.coordinator.close().await.unwrap();

        assert_eq!(context.staging.abort_count(), 2);
        assert_eq!(context.staging.active_count(), 0);
        assert!(matches!(
            context
                .coordinator
                .begin_upload(begin_input("transfer-after-close"))
                .await,
            Err(MobileFileUploadError::Closed)
        ));
        for transfer_id in ["transfer-close-1", "transfer-close-2"] {
            assert!(matches!(
                history(&context, transfer_id).await.last(),
                Some(FileTransferEvent::Cancelled {
                    reason: FileTransferCancellationReason::Unknown,
                    ..
                })
            ));
        }
    }

    #[tokio::test]
    async fn close_waits_for_an_inflight_append_before_cancelling_the_upload() {
        let context = build_context();
        let handle = context
            .coordinator
            .begin_upload(begin_input("transfer-close-waits"))
            .await
            .unwrap();
        context.staging.block_append.store(true, Ordering::SeqCst);
        let append_coordinator = Arc::clone(&context.coordinator);
        let append_handle = handle.clone();
        let append = tokio::spawn(async move {
            append_coordinator
                .append_chunk(&append_handle, b"abcdef")
                .await
        });
        context.staging.append_started.notified().await;

        let close_coordinator = Arc::clone(&context.coordinator);
        let mut close = tokio::spawn(async move { close_coordinator.close().await });
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut close)
            .await
            .is_err());

        context.staging.append_release.notify_one();
        append.await.unwrap().unwrap();
        close.await.unwrap().unwrap();
        assert_eq!(context.staging.abort_count(), 1);
        assert!(matches!(
            history(&context, "transfer-close-waits").await.last(),
            Some(FileTransferEvent::Cancelled {
                reason: FileTransferCancellationReason::Unknown,
                ..
            })
        ));
    }

    #[test]
    fn public_handles_are_opaque_and_unique() {
        let first = MobileFileUploadHandle::new();
        let second = MobileFileUploadHandle::new();

        assert_ne!(first, second);
        assert!(first.as_str().starts_with("uc-mobile-upload-v1:"));
        assert!(!format!("{first:?}").contains(first.as_str()));
    }
}
