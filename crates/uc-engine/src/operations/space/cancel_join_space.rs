use base64::Engine as _;

use crate::error_codes::{CANCEL_JOIN_SPACE_NOT_FOUND_CODE, JOIN_SPACE_FAILED_CODE};
use crate::operations::device::member::join_space_status;
use crate::{CancelJoinSpaceInput, EngineError, EngineErrorCategory, OperationResult};

pub async fn execute_cancel_join_space(
    facade: &uc_application::facade::AppFacade,
    input: CancelJoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input.join_id)
        // 宿主输入校验：无法解码的加入标识按不存在处理，拒绝原因已完整表达。
        .map_err(|_| not_found())?;
    // 宿主输入校验：长度不符的加入标识按不存在处理（错误值只是原字节）。
    let join_id: [u8; 16] = bytes.try_into().map_err(|_| not_found())?;
    facade
        .cancel_space_join(join_id)
        .await
        .map(|status| OperationResult::JoinSpace(join_space_status(status)))
        .map_err(|error| match error {
            uc_application::facade::CancelSpaceJoinError::NotFound => not_found(),
            _ => EngineError::new(JOIN_SPACE_FAILED_CODE, EngineErrorCategory::Internal, false),
        })
}

fn not_found() -> EngineError {
    EngineError::new(
        CANCEL_JOIN_SPACE_NOT_FOUND_CODE,
        EngineErrorCategory::NotFound,
        false,
    )
}
