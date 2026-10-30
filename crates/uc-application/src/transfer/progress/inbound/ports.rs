use super::super::{ProgressExchangeDeadline, ProgressReceiveError};
use crate::connectivity::{ConnectionLease, PeerReceiveStream, StreamLease, UniStreamLease};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// 一条已准入流的完整编解码与业务交接；不能把广播入队当作业务ACK。
#[async_trait]
pub trait ProgressInboundWirePort: Send + Sync {
    async fn current(
        &self,
        stream: StreamLease,
        connection: &ConnectionLease,
        generation: u64,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
    async fn legacy(
        &self,
        stream: UniStreamLease<Box<dyn PeerReceiveStream>>,
        connection: &ConnectionLease,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
}
