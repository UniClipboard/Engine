//! 准入状态端口失败的统一记录：只写固定分类，不渲染错误正文。

use uc_core::error_class::ErrorClass;
use uc_observability_contract::error_source::find_source;

use super::repository::SpaceAdmissionStateStoreError;

/// 来源链上仓储错误的分类；上层端口错误只带本层分类，拒绝原因等细节保存在这一层。
pub(super) fn store_error_class(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    find_source::<SpaceAdmissionStateStoreError>(error).map(ErrorClass::class)
}

/// 结果为错误时记录一条 WARN（本层分类加仓储来源分类），并原样返回结果。
macro_rules! warn_state_failure {
    ($result:expr, $message:literal) => {{
        let result = $result;
        if let ::core::result::Result::Err(error) = &result {
            ::uc_observability_contract::uc_warn!(
                error_class = ::uc_core::error_class::ErrorClass::class(error),
                source_class = $crate::space::admission::failure_log::store_error_class(error),
                $message
            );
        }
        result
    }};
}
pub(super) use warn_state_failure;
