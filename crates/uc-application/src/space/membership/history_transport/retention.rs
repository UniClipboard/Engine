use crate::connectivity::{ConnectionLease, ConnectivityManager};
use tokio::time::{timeout_at, Instant};
use uc_core::error_class::ErrorClass;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

/// 业务处理已结算，保留资格单独决定传输回执的连接提示，不改变业务结果。
pub(super) async fn prepare_connection_retention(
    manager: &ConnectivityManager,
    connection: &ConnectionLease,
    deadline: Instant,
) -> bool {
    match timeout_at(deadline, manager.retain(connection)).await {
        Ok(Ok(retained)) => retained,
        Ok(Err(source)) => {
            manager.retire(connection);
            uc_warn!(
                error_kind = source.class(),
                io_error_kind = io_error_kind(&source),
                "成员历史响应使用临时连接，保留判定失败"
            );
            false
        }
        Err(_deadline) => {
            manager.retire(connection);
            uc_warn!(
                error_kind = "membership_history_retention_timeout",
                "成员历史响应使用临时连接，保留判定超时"
            );
            false
        }
    }
}
