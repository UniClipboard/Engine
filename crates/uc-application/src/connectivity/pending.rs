//! 首帧身份介绍需要有限未绑定使用权；不构造虚假的 DeviceId grant。
use super::connection::ConnectionRecord;
use super::retention::ConnectionActivity;
use super::{
    AdmissionGrant, ConnectionLease, ConnectivityError, ConnectivityManager, InboundAdmissionFence,
    PeerConnection, PeerReceiveStream, PeerSendStream, StreamLease,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

pub(super) struct UnboundConnection {
    pub raw: Arc<dyn PeerConnection>,
    pub adopted: bool,
}
impl Drop for UnboundConnection {
    fn drop(&mut self) {
        if !self.adopted {
            self.raw.close();
        }
    }
}

/// 首帧与身份解析持有同一组容量；失败/取消自动关闭临时连接。
pub struct PendingInboundLease {
    pub send: Box<dyn PeerSendStream>,
    pub receive: Box<dyn PeerReceiveStream>,
    pub deadline: Instant,
    pub(super) received_at: Instant,
    pub(super) raw: UnboundConnection,
    pub(super) manager: ConnectivityManager,
    pub(super) incoming: OwnedSemaphorePermit,
    pub(super) global: OwnedSemaphorePermit,
    pub(super) fence: Option<Box<dyn InboundAdmissionFence>>,
}
impl PendingInboundLease {
    pub fn connection(&self) -> Arc<dyn PeerConnection> {
        self.raw.raw.clone()
    }
    pub async fn bind(
        mut self,
        grant: Box<dyn AdmissionGrant>,
    ) -> Result<(ConnectionLease, StreamLease), ConnectivityError> {
        if let Some(fence) = &self.fence {
            fence.check_current(&grant.peer())?;
        }
        grant.check_current().await?;
        if let Some(fence) = &self.fence {
            fence.check_current(&grant.peer())?;
        }
        let record = Arc::new(ConnectionRecord {
            connection: self.raw.raw.clone(),
            activity: Arc::new(ConnectionActivity::default()),
            stop: self.manager.stop_token(),
            cache_cancel: tokio_util::sync::CancellationToken::new(),
            accepted_stream_timeout: self.manager.policy().accepted_stream_timeout,
            pending: tokio::sync::Mutex::new(None),
            exchange_claimed: AtomicBool::new(true),
            peer: self.manager.peer(grant.peer()),
            grant,
            inbound: Arc::new(Semaphore::new(self.manager.policy().inbound_per_connection)),
            inbound_global: self.manager.inbound_capacity(),
            waiting_global: self.manager.waiting_capacity(),
            _inbound_connection: Some(self.incoming),
        });
        let local = record
            .inbound
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        self.manager.track(&record)?;
        self.raw.adopted = true;
        let mut stream = StreamLease::new(
            (self.send, self.receive),
            record.clone(),
            vec![local, self.global],
        )?;
        stream.received_at = Some(self.received_at);
        Ok((
            ConnectionLease {
                record,
                request: None,
                _activity: None,
            },
            stream,
        ))
    }
}
