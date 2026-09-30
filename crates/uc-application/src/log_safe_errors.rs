//! Application 层拥有的、可安全进入模块日志的错误类型登记。
//!
//! 只登记 `#[error]` 文本为固定文字、枚举变体名或 `Sensitive` 包装值的类型；新增或修改错误类型时在此补充。

use uc_observability_contract::log_safe_errors;
use uc_observability_contract::module_log::register_error_layer_renderers;

use crate::deps::{
    JoinerActivationStateError, JoinerCancellationStateError, JoinerStartStateError,
    MembershipLedgerError, PendingAdmissionRecoveryStateError, SponsorAdmissionStateError,
};
use uc_core::membership::{KeyEpochError, MembershipHistoryV2Error};

use crate::runtime_lifecycle::LifecycleError;
use crate::search::SearchShutdownError;

log_safe_errors!(fn admission_state_errors => [
    SponsorAdmissionStateError,
    JoinerStartStateError,
    JoinerActivationStateError,
    JoinerCancellationStateError,
    PendingAdmissionRecoveryStateError,
    MembershipLedgerError,
    MembershipHistoryV2Error,
    KeyEpochError,
    LifecycleError,
    SearchShutdownError,
]);

/// 登记本 crate 拥有的错误类型；由 Engine 装配统一调用。
pub fn register_log_safe_errors() {
    register_error_layer_renderers(&[admission_state_errors]);
}
