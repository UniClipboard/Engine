use super::connection::ConnectionRecord;
use super::retention::ActivityGuard;
use super::ConnectivityError;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

pub trait PeerSendStream: AsyncWrite + Unpin + Send {
    /// 本连接内的流标识；只用于区分流，不承载业务身份。
    fn id(&self) -> u64;
    /// 立即终止本发送流；不能关闭共用连接。
    fn reset(&mut self, code: u64) -> Result<(), ConnectivityError>;
    fn finish(&mut self) -> Result<(), ConnectivityError>;
    /// FIN 已发出后，等待数据确认或对端停止接受；不代表业务回执被采用。
    fn settled(&mut self) -> BoxFuture<'_, Result<(), ConnectivityError>>;
}
pub trait PeerReceiveStream: AsyncRead + Unpin + Send {
    fn id(&self) -> u64;
    /// 停止本接收流；已开始的本地工作仍由完整交换负责人结算。
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError>;
}
pub type StreamPair = (Box<dyn PeerSendStream>, Box<dyn PeerReceiveStream>);

/// 两半流与完整使用权一起释放；丢弃未完成半流只终止本交换。
pub struct StreamLease {
    pub send: Box<dyn PeerSendStream>,
    pub receive: Box<dyn PeerReceiveStream>,
    /// 原生入站流首次被接收的时间；排队和重新等待不会刷新它。
    pub received_at: Option<Instant>,
    /// 仅由受管入站记录；业务预算在本地准入完成后仍从实际收流起算。
    pub(crate) received_elapsed_at: Option<Duration>,
    _connection: Arc<ConnectionRecord>,
    _permits: Arc<Vec<OwnedSemaphorePermit>>,
    _activity: Arc<ActivityGuard>,
}

/// 已开始本地工作的资源使用权；网络取消后仍计入同一容量，不登记第二套任务。
#[derive(Clone)]
pub struct StreamWorkLease {
    _connection: Arc<ConnectionRecord>,
    _permits: Arc<Vec<OwnedSemaphorePermit>>,
    _activity: Arc<ActivityGuard>,
}

impl StreamWorkLease {
    pub(super) fn new(
        connection: Arc<ConnectionRecord>,
        permits: Vec<OwnedSemaphorePermit>,
    ) -> Result<Self, ConnectivityError> {
        let activity = connection
            .activity
            .acquire(connection.connection.as_ref())?;
        Ok(Self {
            _connection: connection,
            _permits: Arc::new(permits),
            _activity: activity,
        })
    }
}

impl StreamLease {
    /// 只停止本交换；调用方仍负责已经移出的 SDK 工作。
    pub fn stop(mut self) -> Result<(), ConnectivityError> {
        let send = self.send.reset(0);
        let receive = self.receive.stop(0);
        match (send, receive) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(source), Ok(())) | (Ok(()), Err(source)) => Err(source),
            (Err(primary), Err(additional)) => Err(ConnectivityError::Settlement {
                primary: Box::new(primary),
                additional: Box::new(additional),
            }),
        }
    }
    pub(super) fn attach_capacity(&mut self, permits: Vec<OwnedSemaphorePermit>) {
        self._permits = Arc::new(permits);
    }
    pub(crate) fn hold_for_work(&self) -> StreamWorkLease {
        StreamWorkLease {
            _connection: self._connection.clone(),
            _permits: self._permits.clone(),
            _activity: self._activity.clone(),
        }
    }

    /// 将 I/O 交给完整交换能力；工作租约须保留到 SDK 导入／导出与本地结算结束。
    pub fn into_parts(self) -> (StreamPair, StreamWorkLease) {
        let work = StreamWorkLease {
            _connection: self._connection,
            _permits: self._permits,
            _activity: self._activity,
        };
        ((self.send, self.receive), work)
    }

    pub(super) fn new(
        pair: StreamPair,
        connection: Arc<ConnectionRecord>,
        permits: Vec<OwnedSemaphorePermit>,
    ) -> Result<Self, ConnectivityError> {
        let activity = connection
            .activity
            .acquire(connection.connection.as_ref())?;
        Ok(Self {
            send: pair.0,
            receive: pair.1,
            received_at: None,
            received_elapsed_at: None,
            _connection: connection,
            _permits: Arc::new(permits),
            _activity: activity,
        })
    }
}
