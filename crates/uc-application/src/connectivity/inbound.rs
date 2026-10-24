//! 入站准入的同期围栏与容量交接；成员读取结束之前不释放识别后额度。
use super::lifetime::ConnectionLifetime;
use super::manager::PeerState;
use super::pending::UnboundConnection;
use super::{
    ConnectivityError, ConnectivityManager, InboundAdmissionFence, InboundConnectionError,
    InboundIdentityReservation, PeerConnection,
};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

#[derive(Clone, Copy)]
pub struct InboundAdmissionPolicy {
    pub unidentified: usize,
    pub identified_per_peer: usize,
    pub timeout: Duration,
}

struct IdentifiedPeer {
    peer: DeviceId,
    _state: Arc<PeerState>,
    _capacity: Option<OwnedSemaphorePermit>,
}
struct Capacity {
    identified: Option<IdentifiedPeer>,
}
pub(super) struct InboundReservation {
    pub manager: ConnectivityManager,
    pub raw: UnboundConnection,
    pub lifetime: Arc<ConnectionLifetime>,
    pub session: CancellationToken,
    pub deadline: Option<Instant>,
    pub started_at: Option<Instant>,
    fence: Option<Box<dyn InboundAdmissionFence>>,
    capacity: Mutex<Capacity>,
}
impl InboundReservation {
    /// 同步执行，因此尚未 poll 的连接不能跨会话重获准入资格。
    pub fn new(
        manager: ConnectivityManager,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Self, ConnectivityError> {
        let raw = UnboundConnection {
            raw: connection,
            adopted: false,
        };
        let session = manager.stop_token();
        let fence = manager.inner.admission.inbound_fence();
        let started_at = manager.policy().inbound_admission.map(|_| Instant::now());
        let deadline = started_at
            .zip(manager.policy().inbound_admission)
            .map(|(start, limits)| start + limits.timeout);
        if session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let capacity = manager
            .inner
            .incoming
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        let lifetime = ConnectionLifetime::inbound(capacity);
        ConnectivityManager::watch_connection(
            &manager.inner,
            raw.raw.clone(),
            lifetime.clone(),
            session.clone(),
        );
        Ok(Self {
            manager,
            raw,
            lifetime,
            session,
            deadline,
            started_at,
            fence,
            capacity: Mutex::new(Capacity { identified: None }),
        })
    }
    pub fn verify(&self, peer: DeviceId) -> Result<(), ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ConnectivityError::AdmissionDeadlineExpired);
        }
        self.fence.identified(peer)?;
        let capacity = self.capacity.lock().unwrap_or_else(PoisonError::into_inner);
        if capacity
            .identified
            .as_ref()
            .is_none_or(|identified| identified.peer != peer)
        {
            return Err(ConnectivityError::Denied);
        }
        Ok(())
    }
    pub fn take_unidentified(&self) -> Option<OwnedSemaphorePermit> {
        if self.manager.policy().inbound_admission.is_some() {
            None
        } else {
            self.lifetime.take_unidentified()
        }
    }
    pub fn failed(mut self, cause: ConnectivityError) -> InboundConnectionError {
        // 旧剪贴板协议仍由调用方发送有界拒绝回执；blob 直接关闭本次临时连接。
        self.raw.adopted = self.manager.policy().inbound_admission.is_none();
        InboundConnectionError {
            cause,
            _capacity: self.take_unidentified(),
        }
    }
}
impl InboundIdentityReservation for InboundReservation {
    fn identified(&self, peer: DeviceId) -> Result<(), ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ConnectivityError::AdmissionDeadlineExpired);
        }
        self.fence.identified(peer)?;
        let mut capacity = self.capacity.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(identified) = &capacity.identified {
            return if identified.peer == peer {
                Ok(())
            } else {
                Err(ConnectivityError::Denied)
            };
        }
        let state = self.manager.peer(peer);
        let identified_capacity = match self.manager.policy().inbound_admission {
            Some(_) => Some(
                state
                    .pending_inbound
                    .clone()
                    .try_acquire_owned()
                    .map_err(ConnectivityError::Busy)?,
            ),
            None => None,
        };
        self.lifetime.reserve(state.clone(), true)?;
        capacity.identified = Some(IdentifiedPeer {
            peer,
            _state: state,
            _capacity: identified_capacity,
        });
        if self.manager.policy().inbound_admission.is_some() {
            self.lifetime.take_unidentified();
        }
        Ok(())
    }
}
