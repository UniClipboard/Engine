//! 历史记录导出到宿主文件句柄。

use super::*;
use crate::HostCapabilityError;
use crate::HostCapabilityErrorCategory;

impl ProductionRuntime {
    pub(crate) async fn execute_export_entry(
        &self,
        input: ExportEntryInput,
    ) -> Result<OperationResult, EngineError> {
        let facade = self.current_facade().await?;
        let bytes = load_export_bytes(&facade, &input.entry_id).await?;
        for (index, chunk) in bytes.chunks(EXPORT_CHUNK_SIZE).enumerate() {
            let offset = (index as u64) * (EXPORT_CHUNK_SIZE as u64);
            self.files
                .write_chunk(&input.destination, offset, chunk)
                .map_err(map_export_host_error)?;
            tokio::task::yield_now().await;
        }
        self.files
            .finish_write(&input.destination)
            .map_err(map_export_host_error)?;
        Ok(OperationResult::EntryExported)
    }
}

async fn load_export_bytes(facade: &AppFacade, entry_id: &str) -> Result<Vec<u8>, EngineError> {
    let resource = facade
        .get_history_entry_resource(entry_id)
        .await
        .map_err(map_export_history_error)?;
    let file_list = resource.mime_type.as_deref().is_some_and(|mime| {
        mime.eq_ignore_ascii_case("text/uri-list") || mime.eq_ignore_ascii_case("file/uri-list")
    });
    if file_list {
        return facade
            .read_entry_file_resource(entry_id)
            .await
            .map(|file| file.bytes)
            .map_err(map_export_resource_error);
    }
    if let Some(bytes) = resource.inline_data {
        return Ok(bytes);
    }
    if let Some(blob_id) = resource.blob_id {
        return facade
            .read_blob_resource(&blob_id)
            .await
            .map(|blob| blob.bytes)
            .map_err(map_export_resource_error);
    }
    Err(EngineError::new(
        EXPORT_FAILED_CODE,
        EngineErrorCategory::Internal,
        false,
    ))
}

fn map_export_history_error(error: ClipboardHistoryError) -> EngineError {
    match error {
        ClipboardHistoryError::NotFound => {
            EngineError::new(EXPORT_NOT_FOUND_CODE, EngineErrorCategory::NotFound, false)
        }
        ClipboardHistoryError::UnsupportedContent => {
            EngineError::new(EXPORT_FAILED_CODE, EngineErrorCategory::Conflict, false)
        }
        ClipboardHistoryError::Internal(_) => {
            operation_error_with_code(EXPORT_FAILED_CODE, "load export entry", error)
        }
    }
}

fn map_export_resource_error(error: ResourceFacadeError) -> EngineError {
    match error {
        ResourceFacadeError::NotFound => {
            EngineError::new(EXPORT_NOT_FOUND_CODE, EngineErrorCategory::NotFound, false)
        }
        ResourceFacadeError::SessionLocked
        | ResourceFacadeError::Busy
        | ResourceFacadeError::Unavailable
        | ResourceFacadeError::Mismatch(_)
        | ResourceFacadeError::Internal(_) => {
            operation_error_with_code(EXPORT_FAILED_CODE, "load export resource", error)
        }
    }
}

fn map_export_host_error(error: HostCapabilityError) -> EngineError {
    let (code, category, retryable) = match error.category() {
        HostCapabilityErrorCategory::InvalidHandle => (
            EXPORT_INVALID_TARGET_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        HostCapabilityErrorCategory::PermissionDenied => (
            EXPORT_UNAUTHORIZED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        HostCapabilityErrorCategory::Unavailable | HostCapabilityErrorCategory::Io => (
            EXPORT_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
    };
    uc_error!(
        error_kind = "host_export",
        io_error_kind = io_error_kind(&error),
        "host export failed"
    );
    EngineError::new(code, category, retryable)
}
