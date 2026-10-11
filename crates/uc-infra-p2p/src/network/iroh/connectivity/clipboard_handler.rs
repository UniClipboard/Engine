use super::IrohPeerConnection;
use crate::network::clipboard::ClipboardReceiverAdapter;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

/// 唯一原生 handler：密码握手连接转换后交给完整 Uni 接收入口。
#[derive(Clone)]
pub struct IrohClipboardHandler {
    receiver: Arc<ClipboardReceiverAdapter>,
}
impl IrohClipboardHandler {
    pub fn new(receiver: Arc<ClipboardReceiverAdapter>) -> Self {
        Self { receiver }
    }
}
impl std::fmt::Debug for IrohClipboardHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohClipboardHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohClipboardHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.receiver
            .receive(IrohPeerConnection::wrap(connection))
            .await;
        Ok(())
    }
    async fn shutdown(&self) {
        if let Err(error) = self.receiver.shutdown().await {
            uc_warn!(
                error_kind = "clipboard_connectivity_shutdown",
                io_error_kind = io_error_kind(&error),
                "clipboard connectivity shutdown failed"
            );
        }
    }
}
