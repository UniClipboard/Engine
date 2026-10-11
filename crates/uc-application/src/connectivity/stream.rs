use super::connection::ConnectionRecord;
use super::ConnectivityError;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OwnedSemaphorePermit;

pub trait PeerSendStream: AsyncWrite + Unpin + Send {
    fn finish(&mut self) -> Result<(), ConnectivityError>;
}
pub trait PeerReceiveStream: AsyncRead + Unpin + Send {}
pub type StreamPair = (Box<dyn PeerSendStream>, Box<dyn PeerReceiveStream>);

/// 两半流与完整使用权一起释放；丢弃未完成半流只终止本交换。
pub struct StreamLease {
    pub send: Box<dyn PeerSendStream>,
    pub receive: Box<dyn PeerReceiveStream>,
    _connection: Arc<ConnectionRecord>,
    _permits: Vec<OwnedSemaphorePermit>,
}

impl StreamLease {
    pub(super) fn new(
        pair: StreamPair,
        connection: Arc<ConnectionRecord>,
        permits: Vec<OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            send: pair.0,
            receive: pair.1,
            _connection: connection,
            _permits: permits,
        }
    }
}
