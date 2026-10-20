//! Router 一次交接完整入站，网络 bridge 不编排准入或在线步骤。
use super::IrohPeerConnection;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use tokio::time::Instant;
use uc_application::deps::PeerReachabilityTransport;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Clone)]
pub struct IrohPeerReachabilityHandler {
    owner: Arc<PeerReachabilityTransport>,
}
impl IrohPeerReachabilityHandler {
    pub fn new(owner: Arc<PeerReachabilityTransport>) -> Self {
        Self { owner }
    }
}
impl std::fmt::Debug for IrohPeerReachabilityHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohPeerReachabilityHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohPeerReachabilityHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let received_at = Instant::now();
        self.owner
            .accept(IrohPeerConnection::wrap(connection), received_at)
            .await
            .map_err(AcceptError::from_err)
    }
    async fn shutdown(&self) {
        if let Err(source) = self.owner.shutdown().await {
            uc_warn!(
                error_kind = "presence_shutdown_failed",
                io_error_kind = io_error_kind(&source),
                "presence owner shutdown failed"
            );
        }
    }
}
