//! 首帧身份介绍需要有限未绑定使用权；不构造虚假的 DeviceId grant。
use super::connection::ConnectionRecord;
use super::lifetime::ConnectionLifetime;
use super::manager::PeerState;
use super::retention::ConnectionActivity;
use super::{
    AdmissionGrant, ConnectionLease, ConnectivityError, ConnectivityManager, InboundAdmissionFence,
    InboundIdentityReservation, PeerConnection, PeerReceiveStream, PeerSendStream, StreamLease,
};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

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
    pub(super) session: CancellationToken,
    pub(super) lifetime: Arc<ConnectionLifetime>,
    pub(super) incoming: OwnedSemaphorePermit,
    pub(super) global: OwnedSemaphorePermit,
    pub(super) identified_peer: Option<(DeviceId, Arc<PeerState>)>,
    pub(super) identified_capacity: Option<OwnedSemaphorePermit>,
    pub(super) fence: Option<Box<dyn InboundAdmissionFence>>,
}
impl PendingInboundLease {
    pub fn connection(&self) -> Arc<dyn PeerConnection> {
        self.raw.raw.clone()
    }
    /// 已识别身份立即占用每端 pending 额度；不排队，也不占长期集合。
    pub fn reserve_peer(&mut self, peer: DeviceId) -> Result<(), ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if let Some(fence) = &self.fence {
            fence.check_current(&peer)?;
        }
        let state = self.manager.peer(peer);
        self.identified_capacity = Some(
            state
                .pending_inbound
                .clone()
                .try_acquire_owned()
                .map_err(ConnectivityError::Busy)?,
        );
        self.lifetime.reserve(state.clone(), true)?;
        self.identified_peer = Some((peer, state));
        Ok(())
    }
    /// 取得同一会话准入证明；并非建立第二份成员资格。
    pub async fn admit(&mut self) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let reservation = PendingIdentityReservation {
            session: &self.session,
            fence: &self.fence,
            peer: self.identified_peer.as_ref().map(|(peer, _)| *peer),
        };
        self.manager
            .admit_pending(self.raw.raw.clone(), &reservation)
            .await
    }
    pub async fn bind(
        mut self,
        grant: Box<dyn AdmissionGrant>,
    ) -> Result<(ConnectionLease, StreamLease), ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if Instant::now() >= self.deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        if let Some(fence) = &self.fence {
            fence.check_current(&grant.peer())?;
        }
        grant.check_current().await?;
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if Instant::now() >= self.deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        if let Some(fence) = &self.fence {
            fence.check_current(&grant.peer())?;
        }
        let presence = self.manager.policy().presence_retention;
        if presence && self.identified_peer.as_ref().map(|(peer, _)| *peer) != Some(grant.peer()) {
            return Err(ConnectivityError::Denied);
        }
        let mut permits = vec![self.global];
        let incoming = if presence {
            permits.push(self.incoming);
            if let Some(capacity) = self.identified_capacity.take() {
                permits.push(capacity);
            }
            None
        } else {
            Some(self.incoming)
        };
        let record = Arc::new(ConnectionRecord {
            connection: self.raw.raw.clone(),
            address_hint: None,
            activity: Arc::new(ConnectionActivity::default()),
            stop: self.session.clone(),
            cache_cancel: tokio_util::sync::CancellationToken::new(),
            supervise_local_work: self.manager.policy().wait_for_local_work(),
            accepted_stream_timeout: self.manager.policy().accepted_stream_timeout,
            reject_excess_inbound: self.manager.policy().reject_excess_inbound,
            first_inbound_at: Mutex::new(None),
            pending: tokio::sync::Mutex::new(None),
            exchange_claimed: AtomicBool::new(true),
            peer: self.manager.peer(grant.peer()),
            grant,
            inbound: Arc::new(Semaphore::new(self.manager.policy().inbound_per_connection)),
            inbound_global: self.manager.inbound_capacity(),
            waiting_global: self.manager.waiting_capacity(),
            inbound_direction: true,
            _lifetime: self.lifetime.clone(),
            _inbound_connection: incoming,
        });
        let local = record
            .inbound
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        self.manager.track(&record)?;
        self.raw.adopted = true;
        permits.push(local);
        let mut stream = StreamLease::new((self.send, self.receive), record.clone(), permits)?;
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

struct PendingIdentityReservation<'a> {
    session: &'a CancellationToken,
    fence: &'a Option<Box<dyn InboundAdmissionFence>>,
    peer: Option<DeviceId>,
}
impl InboundIdentityReservation for PendingIdentityReservation<'_> {
    fn identified(&self, peer: DeviceId) -> Result<(), ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        self.fence.identified(peer)?;
        if self.peer.is_some_and(|expected| expected != peer) {
            return Err(ConnectivityError::Denied);
        }
        Ok(())
    }
}
