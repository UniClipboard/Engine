//! 对端连接协调器的失败分类与记录。
//!
//! 手动 refresh 的失败只通过响应通道返回，并在公开契约边界丢弃来源；
//! 协调器是该动作的负责人，在这里记录一次固定分类。

use uc_core::error_class::ErrorClass;
use uc_observability_contract::{error_source::io_error_kind, uc_info, uc_warn};

use super::PeerConnectionError;

impl ErrorClass for PeerConnectionError {
    fn class(&self) -> &'static str {
        match self {
            Self::Environment(_) => "environment",
            Self::Closed => "closed",
            Self::Paused => "paused",
            Self::Busy => "busy",
            Self::Scope(_) => "scope",
            Self::ScopeTimeout(_) => "scope_timeout",
            Self::Response(_) => "response",
            Self::Task(_) => "task",
        }
    }
}

/// 记录一次 refresh 失败；协调器已关闭或暂停是可预期的生命周期状态，不按故障记录。
pub(super) fn record_refresh_failure(error: &PeerConnectionError) {
    match error {
        PeerConnectionError::Closed | PeerConnectionError::Paused => uc_info!(
            operation = "refresh_peer_reachability",
            outcome = "rejected",
            error_class = error.class(),
            "peer reachability refresh rejected"
        ),
        _ => uc_warn!(
            operation = "refresh_peer_reachability",
            outcome = "failed",
            error_kind = "refresh",
            error_class = error.class(),
            io_error_kind = io_error_kind(error),
            "peer reachability refresh failed"
        ),
    }
}
