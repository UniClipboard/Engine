use super::connection::ConnectionRecord;
use super::retention::ActivityGuard;
use super::ConnectivityError;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

pub trait PeerSendStream: AsyncWrite + Unpin + Send {
    fn finish(&mut self) -> Result<(), ConnectivityError>;
    /// FIN 已发出后，等待数据确认或对端停止接受；不代表业务回执被采用。
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>>;
}
pub trait PeerReceiveStream: AsyncRead + Unpin + Send {}
pub type StreamPair = (Box<dyn PeerSendStream>, Box<dyn PeerReceiveStream>);

/// 两半流与完整使用权一起释放；丢弃未完成半流只终止本交换。
pub struct StreamLease {
    pub send: Box<dyn PeerSendStream>,
    pub receive: Box<dyn PeerReceiveStream>,
    /// 原生入站流首次被接收的时间；排队和重新等待不会刷新它。
    pub received_at: Option<Instant>,
    _connection: Arc<ConnectionRecord>,
    _permits: Arc<Vec<OwnedSemaphorePermit>>,
    _activity: Arc<ActivityGuard>,
}

/// 已开始本地工作的资源使用权；网络取消后仍计入同一容量，不登记第二套任务。
pub(crate) struct StreamUse {
    _permits: Arc<Vec<OwnedSemaphorePermit>>,
    _activity: Arc<ActivityGuard>,
}

impl StreamLease {
    pub(crate) fn hold_for_work(&self) -> StreamUse {
        StreamUse {
            _permits: self._permits.clone(),
            _activity: self._activity.clone(),
        }
    }

    pub(super) fn new(
        pair: StreamPair,
        connection: Arc<ConnectionRecord>,
        permits: Vec<OwnedSemaphorePermit>,
    ) -> Result<Self, ConnectivityError> {
        let activity = connection.activity.acquire()?;
        Ok(Self {
            send: pair.0,
            receive: pair.1,
            received_at: None,
            _connection: connection,
            _permits: Arc::new(permits),
            _activity: activity,
        })
    }
}
