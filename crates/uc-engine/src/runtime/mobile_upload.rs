use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::warn;
use uc_application::facade::{
    BeginReceiverTransfer, FileTransferSession, ReceiverTransferRegistration,
};
use uc_core::mobile_sync::StagingHandle;
use uc_core::ports::MobileFileStagingError;
use uc_core::FileTransferFailureReason;

use super::ProductionRuntime;
use crate::compatibility::mobile_lan::content_operations::{map_apply_error, map_apply_outcome};
use crate::{
    AppendMobileFileUploadInput, BeginMobileFileUploadInput, EngineError, EngineErrorCategory,
    FinishMobileFileUploadInput, MobileFileUploadHandle, OperationResult,
};

const MOBILE_UPLOAD_INVALID_INPUT_CODE: u32 = 1446;
const MOBILE_UPLOAD_INVALID_HANDLE_CODE: u32 = 1447;
const MOBILE_UPLOAD_FAILED_CODE: u32 = 1448;
const MOBILE_UPLOAD_PROGRESS_THROTTLE: Duration = Duration::from_millis(250);

pub(super) struct ActiveMobileFileUpload {
    staging: StagingHandle,
    data_name: String,
    source_device_id: String,
    transfer_id: String,
    total_bytes: Option<u64>,
    bytes_received: u64,
    last_progress_at: Instant,
    session: Arc<FileTransferSession>,
}

pub(super) type ActiveMobileFileUploadState = Arc<Mutex<Option<ActiveMobileFileUpload>>>;

fn mobile_upload_invalid_input_error() -> EngineError {
    EngineError::new(
        MOBILE_UPLOAD_INVALID_INPUT_CODE,
        EngineErrorCategory::InvalidInput,
        false,
    )
}

pub(super) fn new_mobile_file_upload_handle() -> MobileFileUploadHandle {
    MobileFileUploadHandle::new(format!(
        "uc-mobile-upload-v1:{}",
        uuid::Uuid::new_v4().simple()
    ))
}

fn mobile_upload_peer_id(source_device_id: &str) -> String {
    format!("mobile:{source_device_id}")
}

fn mobile_upload_invalid_handle_error() -> EngineError {
    EngineError::new(
        MOBILE_UPLOAD_INVALID_HANDLE_CODE,
        EngineErrorCategory::NotFound,
        false,
    )
}

fn map_mobile_upload_error(_error: MobileFileStagingError) -> EngineError {
    mobile_upload_failed_error()
}

fn mobile_upload_failed_error() -> EngineError {
    EngineError::new(
        MOBILE_UPLOAD_FAILED_CODE,
        EngineErrorCategory::Internal,
        true,
    )
}

impl ProductionRuntime {
    pub(super) async fn begin_mobile_file_upload(
        &self,
        input: BeginMobileFileUploadInput,
    ) -> Result<OperationResult, EngineError> {
        if input.data_name.trim().is_empty()
            || input.media_type.trim().is_empty()
            || input.source_device_id.trim().is_empty()
            || input.transfer_id.trim().is_empty()
        {
            return Err(mobile_upload_invalid_input_error());
        }
        let facade = self.current_mobile_sync().await?;
        let session = self.begin_mobile_file_upload_lifecycle(&input).await?;
        let scope_id = uc_application::facade::mobile_sync_streaming_scope_nonce();
        let staging = match facade
            .begin_file_upload(&scope_id, &input.data_name, &input.media_type)
            .await
        {
            Ok(staging) => staging,
            Err(error) => {
                self.fail_mobile_file_upload_lifecycle(&session, "mobile upload staging failed")
                    .await;
                return Err(map_mobile_upload_error(error));
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
        let public_handle = loop {
            let candidate = new_mobile_file_upload_handle();
            let mut uploads = self.mobile_file_uploads.lock().await;
            if !uploads.contains_key(candidate.as_str()) {
                uploads.insert(candidate.as_str().to_string(), Arc::clone(&upload));
                break candidate;
            }
        };
        Ok(OperationResult::MobileFileUploadStarted(public_handle))
    }

    async fn begin_mobile_file_upload_lifecycle(
        &self,
        input: &BeginMobileFileUploadInput,
    ) -> Result<Arc<FileTransferSession>, EngineError> {
        let peer_id = mobile_upload_peer_id(&input.source_device_id);
        let session = self
            .file_transfer_facade
            .begin_receiver_transfer(BeginReceiverTransfer {
                transfer_id: input.transfer_id.clone(),
                peer_id,
                filename: input.data_name.clone(),
                file_size: input.total_bytes,
                registration: ReceiverTransferRegistration::Provisional,
            })
            .await
            .map_err(|_| mobile_upload_failed_error())?;
        if session.report_progress(0, input.total_bytes).await.is_err() {
            self.fail_mobile_file_upload_lifecycle(&session, "mobile upload progress failed")
                .await;
            return Err(mobile_upload_failed_error());
        }
        Ok(session)
    }

    async fn report_mobile_file_upload_progress(
        &self,
        session: &FileTransferSession,
        bytes_received: u64,
        total_bytes: Option<u64>,
    ) -> Result<(), EngineError> {
        session
            .report_progress(bytes_received, total_bytes)
            .await
            .map(|_| ())
            .map_err(|_| mobile_upload_failed_error())
    }

    async fn fail_mobile_file_upload_lifecycle(
        &self,
        session: &FileTransferSession,
        detail: &'static str,
    ) {
        if self
            .try_fail_mobile_file_upload_lifecycle(session, detail)
            .await
            .is_err()
        {
            warn!("mobile upload failure notification failed");
        }
    }

    async fn try_fail_mobile_file_upload_lifecycle(
        &self,
        session: &FileTransferSession,
        detail: &'static str,
    ) -> Result<(), EngineError> {
        session
            .fail(FileTransferFailureReason::Unknown, Some(detail.to_owned()))
            .await
            .map(|_| ())
            .map_err(|_| mobile_upload_failed_error())
    }

    async fn remove_mobile_file_upload_if_registered(
        &self,
        handle: &MobileFileUploadHandle,
        expected: &ActiveMobileFileUploadState,
    ) {
        let mut uploads = self.mobile_file_uploads.lock().await;
        if uploads
            .get(handle.as_str())
            .is_some_and(|registered| Arc::ptr_eq(registered, expected))
        {
            uploads.remove(handle.as_str());
        }
    }

    pub(super) async fn append_mobile_file_upload(
        &self,
        input: AppendMobileFileUploadInput,
    ) -> Result<OperationResult, EngineError> {
        let appended_bytes =
            u64::try_from(input.bytes.len()).map_err(|_| mobile_upload_invalid_input_error())?;
        let facade = self.current_mobile_sync().await?;
        let upload = self
            .mobile_file_uploads
            .lock()
            .await
            .get(input.handle.as_str())
            .cloned()
            .ok_or_else(mobile_upload_invalid_handle_error)?;
        let mut active = upload.lock().await;
        let staging = active
            .as_ref()
            .map(|active| active.staging.clone())
            .ok_or_else(mobile_upload_invalid_handle_error)?;
        if facade
            .append_file_chunk(&staging, &input.bytes)
            .await
            .is_err()
        {
            let failed = active.take();
            drop(active);
            self.remove_mobile_file_upload_if_registered(&input.handle, &upload)
                .await;
            if let Some(failed) = failed {
                facade.abort_file_upload(failed.staging).await;
                self.fail_mobile_file_upload_lifecycle(
                    &failed.session,
                    "mobile upload append failed",
                )
                .await;
            }
            return Err(mobile_upload_failed_error());
        }

        let progress = active.as_mut().and_then(|active| {
            active.bytes_received = active.bytes_received.saturating_add(appended_bytes);
            (active.last_progress_at.elapsed() >= MOBILE_UPLOAD_PROGRESS_THROTTLE).then(|| {
                (
                    Arc::clone(&active.session),
                    active.bytes_received,
                    active.total_bytes,
                )
            })
        });
        if let Some((session, bytes_received, total_bytes)) = progress {
            if self
                .report_mobile_file_upload_progress(&session, bytes_received, total_bytes)
                .await
                .is_err()
            {
                let failed = active.take();
                drop(active);
                self.remove_mobile_file_upload_if_registered(&input.handle, &upload)
                    .await;
                if let Some(failed) = failed {
                    facade.abort_file_upload(failed.staging).await;
                    self.fail_mobile_file_upload_lifecycle(
                        &failed.session,
                        "mobile upload progress failed",
                    )
                    .await;
                }
                return Err(mobile_upload_failed_error());
            }
            if let Some(active) = active.as_mut() {
                active.last_progress_at = Instant::now();
            }
        }
        Ok(OperationResult::MobileFileUploadChunkAppended)
    }

    pub(super) async fn finish_mobile_file_upload(
        &self,
        input: FinishMobileFileUploadInput,
    ) -> Result<OperationResult, EngineError> {
        if input.media_type.trim().is_empty() {
            return Err(mobile_upload_invalid_input_error());
        }
        let facade = self.current_mobile_sync().await?;
        let upload = self
            .mobile_file_uploads
            .lock()
            .await
            .remove(input.handle.as_str())
            .ok_or_else(mobile_upload_invalid_handle_error)?;
        let upload = upload
            .lock()
            .await
            .take()
            .ok_or_else(mobile_upload_invalid_handle_error)?;
        if self
            .report_mobile_file_upload_progress(
                &upload.session,
                upload.bytes_received,
                upload.total_bytes.or(Some(upload.bytes_received)),
            )
            .await
            .is_err()
        {
            facade.abort_file_upload(upload.staging).await;
            self.fail_mobile_file_upload_lifecycle(
                &upload.session,
                "mobile upload progress failed",
            )
            .await;
            return Err(mobile_upload_failed_error());
        }
        let outcome = match facade
            .finalize_file_upload(
                upload.staging,
                upload.data_name,
                input.media_type,
                uc_core::mobile_sync::MobileDeviceId::new(upload.source_device_id.clone()),
                upload.transfer_id.clone(),
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                self.fail_mobile_file_upload_lifecycle(
                    &upload.session,
                    "mobile upload finalize failed",
                )
                .await;
                return Err(map_apply_error(error));
            }
        };
        Ok(OperationResult::MobileFileUploadFinished(
            map_apply_outcome(outcome),
        ))
    }

    pub(super) async fn abort_mobile_file_upload(
        &self,
        handle: MobileFileUploadHandle,
    ) -> Result<OperationResult, EngineError> {
        let facade = self.current_mobile_sync().await?;
        let upload = self
            .mobile_file_uploads
            .lock()
            .await
            .remove(handle.as_str());
        let existed = upload.is_some();
        if let Some(upload) = upload {
            if let Some(upload) = upload.lock().await.take() {
                facade.abort_file_upload(upload.staging).await;
                self.try_fail_mobile_file_upload_lifecycle(
                    &upload.session,
                    "mobile upload aborted",
                )
                .await?;
            }
        }
        Ok(OperationResult::MobileFileUploadAborted { existed })
    }

    pub(super) async fn abort_all_mobile_file_uploads(
        &self,
        facade: &uc_application::facade::MobileSyncFacade,
    ) {
        let uploads = std::mem::take(&mut *self.mobile_file_uploads.lock().await);
        for upload in uploads.into_values() {
            if let Some(upload) = upload.lock().await.take() {
                facade.abort_file_upload(upload.staging).await;
                self.fail_mobile_file_upload_lifecycle(
                    &upload.session,
                    "mobile upload lifecycle stopped",
                )
                .await;
            }
        }
    }
}
