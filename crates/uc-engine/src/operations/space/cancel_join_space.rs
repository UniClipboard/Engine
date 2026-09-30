use base64::Engine as _;
use uc_application::facade::CancelSpaceJoinError;
use uc_observability_contract::{error_source::io_error_kind, uc_error};

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
        .map_err(map_cancel_join_error)
}

fn map_cancel_join_error(error: CancelSpaceJoinError) -> EngineError {
    match error {
        CancelSpaceJoinError::NotFound => not_found(),
        CancelSpaceJoinError::State { .. } => {
            uc_error!(
                error_kind = "cancel_join_space",
                io_error_kind = io_error_kind(&error),
                "cancel join space failed"
            );
            EngineError::new(JOIN_SPACE_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
    }
}

fn not_found() -> EngineError {
    EngineError::new(
        CANCEL_JOIN_SPACE_NOT_FOUND_CODE,
        EngineErrorCategory::NotFound,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_failure_is_recorded_once_and_a_missing_join_stays_silent() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();

        let missing = map_cancel_join_error(CancelSpaceJoinError::NotFound);
        let failed = map_cancel_join_error(CancelSpaceJoinError::State {
            source: anyhow::Error::new(std::io::Error::other("PRIVATE_STATE")),
        });

        assert_eq!(missing.code(), CANCEL_JOIN_SPACE_NOT_FOUND_CODE);
        assert_eq!(failed.code(), JOIN_SPACE_FAILED_CODE);
        assert_eq!(logs.count("cancel join space failed"), 1);
        assert!(logs.output().contains("io_error_kind=Other"));
        assert!(!logs.output().contains("PRIVATE"));
    }
}
