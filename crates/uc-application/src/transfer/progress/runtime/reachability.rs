//! 已验证在线只是原通知的重试机会；资格、次数与过期仍由完整交换负责人决定。
use crate::transfer::progress::ProgressExchangeService;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;
use uc_core::ports::{PeerReachabilityChanged, ReachabilityState};

pub(in super::super) async fn observe_reachability(
    mut events: broadcast::Receiver<PeerReachabilityChanged>,
    exchanges: ProgressExchangeService,
    cancel: CancellationToken,
) {
    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            event = events.recv() => event,
        };
        match event {
            Ok(event) if event.state == ReachabilityState::Online => {
                exchanges.network_recovered(Some(event.device_id));
            }
            // 丢失提示只合并一次全局机会，不重建原意图、预算或已退休任务。
            Err(RecvError::Lagged(_)) => exchanges.network_recovered(None),
            Ok(_) => {}
            Err(RecvError::Closed) => return,
        }
    }
}
