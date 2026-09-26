use tracing::error;
use uc_application::facade::AppFacade;

use crate::error_codes::QUERY_ACTIVE_CLIPBOARD_FAILED_CODE;
use crate::{ActiveClipboardSummary, EngineError, EngineErrorCategory, OperationResult};

pub async fn execute_query_active_clipboard(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    // 公开契约边界：只产出稳定错误码，失败分类由完整负责人的完成记录提取（见错误处理规范）。
    let active = facade.current_active_clipboard().await.map_err(|_| {
        error!("active clipboard query failed");
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
