use base64::Engine as _;
use uc_application::facade::CancelSpaceJoinError;

use crate::error_codes::{CANCEL_JOIN_SPACE_NOT_FOUND_CODE, JOIN_SPACE_FAILED_CODE};
use crate::operations::device::member::join_space_status;
use crate::{CancelJoinSpaceInput, EngineError, EngineErrorCategory, OperationResult};

pub async fn execute_cancel_join_space(
    facade: &uc_application::facade::AppFacade,
    input: CancelJoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input.join_id)
        // discarded-source[input-validation]: `base64::DecodeError`: the rejection reason is fully expressed by the target classification
        .map_err(|_| not_found())?;
    // discarded-source[no-information]: the error value carries no usable diagnostic information
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
        // 失败记录由取消加入的流程负责人写；这里只做稳定错误码映射。
        CancelSpaceJoinError::State { .. } => {
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
    fn a_state_failure_maps_to_a_stable_code_without_a_second_record() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();

        let missing = map_cancel_join_error(CancelSpaceJoinError::NotFound);
        let failed = map_cancel_join_error(CancelSpaceJoinError::State {
            source: anyhow::Error::new(std::io::Error::other("PRIVATE_STATE")),
        });

        assert_eq!(missing.code(), CANCEL_JOIN_SPACE_NOT_FOUND_CODE);
        assert_eq!(failed.code(), JOIN_SPACE_FAILED_CODE);
        assert_eq!(logs.count("cancel"), 0, "{}", logs.output());
    }
}
