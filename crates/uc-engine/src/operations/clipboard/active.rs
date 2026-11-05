//! 当前生效内容的读取、立即捕获与恢复。
//!
//! 捕获与查询共用“系统剪贴板当前内容”这一事实；恢复在独立的 `restore` 文件里，
//! 因为它还要处理代表选择与写入模式。

use crate::error_codes::*;

use uc_application::facade::{AppFacade, ClipboardCaptureFacadeError};

use crate::{ActiveClipboardSummary, EngineError, EngineErrorCategory, OperationResult};
use uc_observability_contract::uc_error;

pub async fn execute_capture_current_clipboard(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let entry_id = facade
        .capture_current_clipboard()
        .await
        .map_err(map_capture_error)?
        .map(|captured| captured.entry_id);
    Ok(OperationResult::ClipboardCaptured { entry_id })
}

fn map_capture_error(_error: ClipboardCaptureFacadeError) -> EngineError {
    uc_error!("current clipboard capture failed");
    EngineError::new(
        CAPTURE_CURRENT_CLIPBOARD_FAILED_CODE,
        EngineErrorCategory::Internal,
        false,
    )
}

pub async fn execute_query_active_clipboard(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
    let active = facade.current_active_clipboard().await.map_err(|_| {
        EngineError::new(
            QUERY_ACTIVE_CLIPBOARD_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
        )
    })?;

    Ok(OperationResult::ActiveClipboard(active.map(|state| {
        ActiveClipboardSummary {
            entry_id: state.entry_id.as_str().to_owned(),
            activated_by: state.activated_by.as_str().to_owned(),
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_failure_does_not_expose_internal_details() {
        let error = map_capture_error(ClipboardCaptureFacadeError::Internal(anyhow::anyhow!(
            "/private/path/clipboard-cache"
        )));

        assert_eq!(error.category(), EngineErrorCategory::Internal);
        assert_eq!(error.code(), CAPTURE_CURRENT_CLIPBOARD_FAILED_CODE);
        assert!(!error.to_string().contains("clipboard-cache"));
    }
}
