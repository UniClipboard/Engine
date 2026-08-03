use uc_application::facade::{
    BeginMobileFileUpload, MobileFileUploadError,
    MobileFileUploadHandle as ApplicationMobileFileUploadHandle,
};
use uc_core::mobile_sync::MobileDeviceId;

use super::ProductionRuntime;
use crate::compatibility::mobile_lan::content_operations::{map_apply_error, map_apply_outcome};
use crate::{
    AppendMobileFileUploadInput, BeginMobileFileUploadInput, EngineError, EngineErrorCategory,
    FinishMobileFileUploadInput, MobileFileUploadHandle, OperationResult,
};

const MOBILE_UPLOAD_INVALID_INPUT_CODE: u32 = 1446;
const MOBILE_UPLOAD_INVALID_HANDLE_CODE: u32 = 1447;
const MOBILE_UPLOAD_FAILED_CODE: u32 = 1448;

fn map_mobile_upload_error(error: MobileFileUploadError) -> EngineError {
    match error {
        MobileFileUploadError::InvalidInput => EngineError::new(
            MOBILE_UPLOAD_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        MobileFileUploadError::UnknownHandle => EngineError::new(
            MOBILE_UPLOAD_INVALID_HANDLE_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        MobileFileUploadError::CompletionFailed(error) => map_apply_error(error),
        MobileFileUploadError::Closed
        | MobileFileUploadError::Unavailable
        | MobileFileUploadError::UploadFailed => EngineError::new(
            MOBILE_UPLOAD_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
        ),
    }
}

impl ProductionRuntime {
    pub(super) async fn begin_mobile_file_upload(
        &self,
        input: BeginMobileFileUploadInput,
    ) -> Result<OperationResult, EngineError> {
        let facade = self.current_mobile_sync().await?;
        let handle = facade
            .begin_mobile_file_upload(BeginMobileFileUpload {
                data_name: input.data_name,
                media_type: input.media_type,
                source_device_id: MobileDeviceId::new(input.source_device_id),
                transfer_id: input.transfer_id,
                total_bytes: input.total_bytes,
            })
            .await
            .map_err(map_mobile_upload_error)?;
        Ok(OperationResult::MobileFileUploadStarted(
            MobileFileUploadHandle::new(handle.into_string()),
        ))
    }

    pub(super) async fn append_mobile_file_upload(
        &self,
        input: AppendMobileFileUploadInput,
    ) -> Result<OperationResult, EngineError> {
        let facade = self.current_mobile_sync().await?;
        let handle = ApplicationMobileFileUploadHandle::from_string(input.handle.as_str());
        facade
            .append_mobile_file_upload(&handle, &input.bytes)
            .await
            .map_err(map_mobile_upload_error)?;
        Ok(OperationResult::MobileFileUploadChunkAppended)
    }

    pub(super) async fn finish_mobile_file_upload(
        &self,
        input: FinishMobileFileUploadInput,
    ) -> Result<OperationResult, EngineError> {
        let facade = self.current_mobile_sync().await?;
        let handle = ApplicationMobileFileUploadHandle::from_string(input.handle.as_str());
        let outcome = facade
            .finish_mobile_file_upload(handle, input.media_type)
            .await
            .map_err(map_mobile_upload_error)?;
        Ok(OperationResult::MobileFileUploadFinished(
            map_apply_outcome(outcome),
        ))
    }

    pub(super) async fn abort_mobile_file_upload(
        &self,
        handle: MobileFileUploadHandle,
    ) -> Result<OperationResult, EngineError> {
        let facade = self.current_mobile_sync().await?;
        let handle = ApplicationMobileFileUploadHandle::from_string(handle.as_str());
        let existed = facade
            .abort_mobile_file_upload(handle)
            .await
            .map_err(map_mobile_upload_error)?;
        Ok(OperationResult::MobileFileUploadAborted { existed })
    }
}
