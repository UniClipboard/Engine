//! Router 只交接原生连接；准入、交换与关闭由 Application 负责人完成。
use super::super::connectivity::IrohPeerConnection;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use uc_application::deps::{BlobShutdownWait, BlobTransport};
use uc_core::error_class::ErrorClass;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

pub struct IrohBlobHandler {
    owner: Arc<BlobTransport>,
}
impl IrohBlobHandler {
    pub fn new(owner: Arc<BlobTransport>) -> Self {
        Self { owner }
    }
}
impl std::fmt::Debug for IrohBlobHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohBlobHandler").finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohBlobHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.owner
            .serve(IrohPeerConnection::wrap(connection))
            .await
            .map_err(AcceptError::from_err)
    }
    async fn shutdown(&self) {
        if let Err(source) = self.owner.shutdown(BlobShutdownWait::Complete).await {
            uc_warn!(
                error_kind = "blob_transport_shutdown",
                error_class = source.class(),
                io_error_kind = io_error_kind(&source),
                "blob transport shutdown failed"
            );
        }
    }
}
