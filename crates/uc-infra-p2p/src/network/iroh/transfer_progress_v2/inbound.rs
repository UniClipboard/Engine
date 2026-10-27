use super::TransferProgressInbound;
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, PeerReceiveStream, ProgressExchangeDeadline, ProgressInboundWirePort,
    ProgressReceiveError, ProgressReceiverPort, ProgressSnapshotProviderPort,
    ProgressUnattributedReceiverPort, StreamLease, UniStreamLease,
};

/// 已准入单流的协议实现；新版响应只来自Application采用，旧版只交接无关联通知。
pub struct TransferProgressInboundAdapter {
    receiver: Arc<dyn ProgressReceiverPort>,
    legacy: Arc<dyn ProgressUnattributedReceiverPort>,
    queries: Arc<dyn ProgressSnapshotProviderPort>,
}
impl TransferProgressInboundAdapter {
    pub fn new(
        receiver: Arc<dyn ProgressReceiverPort>,
        legacy: Arc<dyn ProgressUnattributedReceiverPort>,
        queries: Arc<dyn ProgressSnapshotProviderPort>,
    ) -> Self {
        Self {
            receiver,
            legacy,
            queries,
        }
    }
}
#[async_trait]
impl ProgressInboundWirePort for TransferProgressInboundAdapter {
    async fn current(
        &self,
        stream: StreamLease,
        connection: &ConnectionLease,
        generation: u64,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        TransferProgressInbound::serve(
            stream,
            connection,
            generation,
            deadline,
            cancel,
            self.receiver.as_ref(),
            self.queries.as_ref(),
        )
        .await
    }
    async fn legacy(
        &self,
        stream: UniStreamLease<Box<dyn PeerReceiveStream>>,
        connection: &ConnectionLease,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        TransferProgressInbound::serve_legacy(
            stream,
            connection,
            deadline,
            cancel,
            self.legacy.as_ref(),
        )
        .await
    }
}
