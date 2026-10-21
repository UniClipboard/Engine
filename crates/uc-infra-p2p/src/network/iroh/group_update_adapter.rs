//! 组更新的 Uni 传输能力；接收业务由 Application 完整 owner 负责。
mod local_work;
mod wire;

use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use uc_application::deps::{ConnectivityManager, GroupUpdateReceiverPort, GroupUpdateWirePort};
use uc_core::membership::{GroupUpdateDispatchError, GroupUpdateDispatchPort, PendingGroupUpdate};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

use super::connectivity::IrohPeerConnection;
pub use local_work::SessionGroupUpdateLocalWork;
pub use uc_sync_protocol::group_update::{GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN};
pub use wire::GroupUpdateWire;

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct IrohGroupUpdateAdapter {
    manager: ConnectivityManager,
    wire: Arc<dyn GroupUpdateWirePort>,
}
impl IrohGroupUpdateAdapter {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn GroupUpdateWirePort>) -> Self {
        Self { manager, wire }
    }
}
#[async_trait]
impl GroupUpdateDispatchPort for IrohGroupUpdateAdapter {
    async fn dispatch_group_update(
        &self,
        update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError> {
        let deadline = tokio::time::Instant::now() + EXCHANGE_TIMEOUT;
        tokio::time::timeout_at(deadline, async {
            let connection = self
                .manager
                .request(*update.recipient())
                .connect(deadline, &CancellationToken::new())
                .await
                .map_err(GroupUpdateDispatchError::offline_from)?;
            self.wire
                .is_supported(connection.protocol())
                .map_err(GroupUpdateDispatchError::transport_from)?;
            let mut stream = connection
                .open_bi(deadline)
                .await
                .map_err(GroupUpdateDispatchError::transport_from)?;
            connection
                .check_current()
                .await
                .map_err(GroupUpdateDispatchError::transport_from)?;
            let result = self
                .wire
                .send(&mut stream, connection.protocol(), update.payload())
                .await;
            connection
                .check_current()
                .await
                .map_err(GroupUpdateDispatchError::transport_from)?;
            connection.close();
            result
        })
        .await
        .map_err(GroupUpdateDispatchError::transport_from)?
    }
}

#[derive(Clone)]
pub struct IrohGroupUpdateHandler {
    manager: ConnectivityManager,
    receiver: Arc<dyn GroupUpdateReceiverPort>,
}
impl IrohGroupUpdateHandler {
    pub fn new(manager: ConnectivityManager, receiver: Arc<dyn GroupUpdateReceiverPort>) -> Self {
        Self { manager, receiver }
    }
}
impl std::fmt::Debug for IrohGroupUpdateHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohGroupUpdateHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohGroupUpdateHandler {
    async fn accept(&self, native: Connection) -> Result<(), AcceptError> {
        let raw = IrohPeerConnection::wrap(native);
        match tokio::time::timeout(EXCHANGE_TIMEOUT, self.manager.accept(raw.clone())).await {
            Ok(Ok(connection)) => {
                if let Err(source) = self.receiver.receive(connection.clone()).await {
                    uc_warn!(
                        error_kind = "group_update_receive",
                        io_error_kind = io_error_kind(source.as_ref()),
                        "组更新交换失败"
                    );
                }
                // 完整接收已在 ACK 后结束；有界清理不计入业务完成耗时。
                let _ = tokio::time::timeout(EXCHANGE_TIMEOUT, connection.closed()).await;
                connection.close();
            }
            _ => raw.close(),
        }
        Ok(())
    }
    async fn shutdown(&self) {
        if let Err(source) = self.manager.shutdown().await {
            uc_warn!(
                error_kind = "group_update_shutdown",
                io_error_kind = io_error_kind(&source),
                "组更新连接关闭失败"
            );
        }
    }
}
