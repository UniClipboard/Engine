//! 拉取客户端拥有一次请求/响应；收到完整封套后关闭单次连接，不重送请求。
use super::error::ActivePullRemoteRefusal;
use super::{
    ActiveClipboardTransportError, ActiveClipboardWirePort, ActivePullResponse,
    ACTIVE_EXCHANGE_TIMEOUT,
};
use crate::connectivity::ConnectivityManager;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uc_core::ids::DeviceId;
use uc_core::ports::{ActiveClipboardPullClientError, ActiveClipboardPullClientPort};

pub struct ActivePullClient {
    manager: ConnectivityManager,
    wire: Arc<dyn ActiveClipboardWirePort>,
}
impl ActivePullClient {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn ActiveClipboardWirePort>) -> Self {
        Self { manager, wire }
    }
}
#[async_trait]
impl ActiveClipboardPullClientPort for ActivePullClient {
    #[instrument(name = "active_state.pull", skip_all)]
    async fn pull(
        &self,
        peer: &DeviceId,
        snapshot_hash: &str,
    ) -> Result<Vec<u8>, ActiveClipboardPullClientError> {
        let deadline = Instant::now() + ACTIVE_EXCHANGE_TIMEOUT;
        let response = timeout_at(deadline, async {
            let connection = self
                .manager
                .request(*peer)
                .connect(deadline, &CancellationToken::new())
                .await?;
            let mut stream = connection.open_bi(deadline).await?;
            self.wire
                .write_pull_request(stream.send.as_mut(), snapshot_hash)
                .await?;
            stream.send.finish()?;
            let response = self
                .wire
                .read_pull_response(stream.receive.as_mut())
                .await?;
            connection.check_current().await?;
            connection.close();
            Ok::<_, ActiveClipboardTransportError>(response)
        })
        .await
        .map_err(ActiveClipboardTransportError::from)??;
        match response {
            ActivePullResponse::Envelope(envelope) => Ok(envelope),
            ActivePullResponse::NotAvailable => Err(ActivePullRemoteRefusal::NotAvailable.into()),
            ActivePullResponse::Locked => Err(ActivePullRemoteRefusal::Locked.into()),
            ActivePullResponse::Internal => Err(ActivePullRemoteRefusal::Internal.into()),
        }
    }
}
