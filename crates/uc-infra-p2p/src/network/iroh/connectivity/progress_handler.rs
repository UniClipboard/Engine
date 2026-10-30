use super::IrohPeerConnection;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::error::Error;
use std::sync::Arc;
use uc_application::deps::TransferProgressTransport;
use uc_core::error_class::ErrorClass;
use uc_observability_contract::uc_warn;

/// Router一次交接完整连接，不编排准入、版本选择、采用或ACK。
#[derive(Clone)]
pub struct IrohTransferProgressHandler {
    owner: Arc<TransferProgressTransport>,
}
impl IrohTransferProgressHandler {
    pub fn new(owner: Arc<TransferProgressTransport>) -> Self {
        Self { owner }
    }
}
impl std::fmt::Debug for IrohTransferProgressHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohTransferProgressHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohTransferProgressHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.owner
            .accept(IrohPeerConnection::wrap(connection))
            .await
            .map_err(AcceptError::from_err)
    }
    async fn shutdown(&self) {
        if let Err(source) = self.owner.shutdown().await {
            uc_warn!(
                error_class = source.class(),
                error = &source as &dyn Error,
                "transfer progress transport shutdown failed"
            );
        }
    }
}
