//! 历史完整负责人给本轮所有交换同一个截止点；不将业务预算泄露到 Core 消息。
use crate::connectivity::ConnectivityError;
use std::future::Future;
use tokio::time::Instant;
use uc_core::membership::MembershipHistoryExchangeError;

tokio::task_local! { static HISTORY_DEADLINE: Instant; }

pub(crate) async fn within_history_budget<F: Future>(deadline: Instant, future: F) -> F::Output {
    HISTORY_DEADLINE.scope(deadline, future).await
}
pub(super) fn exchange_deadline(default: Instant) -> Instant {
    HISTORY_DEADLINE
        .try_with(|deadline| (*deadline).min(default))
        .unwrap_or(default)
}

pub(super) fn ensure_live(deadline: Instant) -> Result<(), MembershipHistoryExchangeError> {
    if Instant::now() >= deadline {
        Err(MembershipHistoryExchangeError::transport_from(
            ConnectivityError::DeadlineExpired,
        ))
    } else {
        Ok(())
    }
}
