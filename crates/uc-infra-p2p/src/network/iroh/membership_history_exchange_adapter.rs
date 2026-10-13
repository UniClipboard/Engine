//! 历史 transport 包装；完整交换与入站业务归 Application 成员模块。
mod address;
mod admission;
mod local_work;
#[cfg(test)]
mod network_tests;
#[cfg(test)]
mod tests;
mod wire;

pub use address::IrohVerifiedPeerAddressRefresh;
pub use admission::HistoryConnectionAdmission;
pub use local_work::SessionMembershipHistoryLocalWork;
pub use uc_sync_protocol::membership_history::{
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
pub use wire::MembershipHistoryWire;

use super::connectivity::IrohPeerConnection;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use std::time::Duration;
use uc_application::deps::{ConnectivityManager, HistoryReceiverPort};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Clone)]
pub struct IrohMembershipHistoryExchangeHandler {
    manager: ConnectivityManager,
    receiver: Arc<dyn HistoryReceiverPort>,
}
impl IrohMembershipHistoryExchangeHandler {
    pub fn new(manager: ConnectivityManager, receiver: Arc<dyn HistoryReceiverPort>) -> Self {
        Self { manager, receiver }
    }
}
impl std::fmt::Debug for IrohMembershipHistoryExchangeHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohMembershipHistoryExchangeHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohMembershipHistoryExchangeHandler {
    async fn accept(&self, native: Connection) -> Result<(), AcceptError> {
        let raw = IrohPeerConnection::wrap(native);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        match self.manager.accept_pending(raw.clone(), deadline).await {
            Ok(pending) => {
                if let Err(source) = self.receiver.receive(pending).await {
                    uc_warn!(
                        error_kind = "membership_history_receive",
                        io_error_kind = io_error_kind(&source),
                        "成员历史交换失败"
                    );
                }
            }
            Err(source) => {
                uc_warn!(
                    error_kind = "membership_history_accept",
                    io_error_kind = io_error_kind(&source),
                    "成员历史入站连接不可用"
                );
                raw.close();
            }
        }
        Ok(())
    }
    async fn shutdown(&self) {
        if let Err(source) = self.manager.shutdown().await {
            uc_warn!(
                error_kind = "membership_history_shutdown",
                io_error_kind = io_error_kind(&source),
                "成员历史连接关闭失败"
            );
        }
    }
}
