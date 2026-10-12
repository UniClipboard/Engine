use super::manager::PeerState;
use super::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectionProtocol, ConnectivityError,
    PeerConnection, StreamLease,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{timeout_at, Instant};
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;

pub(super) struct ConnectionRecord {
    pub connection: Arc<dyn PeerConnection>,
    pub exchange_claimed: AtomicBool,
    pub grant: Box<dyn AdmissionGrant>,
    pub peer: Arc<PeerState>,
    pub inbound: Arc<Semaphore>,
    pub inbound_global: Arc<Semaphore>,
    pub waiting_global: Arc<Semaphore>,
    pub _inbound_connection: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl Drop for ConnectionRecord {
    fn drop(&mut self) {
        self.connection.close();
    }
}

/// 活动使用权；manager 仅保存弱引用，不保留空闲连接。
#[derive(Clone)]
pub struct ConnectionLease {
    pub(super) record: Arc<ConnectionRecord>,
    pub(super) request: Option<Arc<dyn AdmissionAttempt>>,
}

impl ConnectionLease {
    pub fn peer(&self) -> DeviceId {
        self.record.grant.peer()
    }
    pub fn mode(&self) -> ConnectionMode {
        self.record.connection.mode()
    }
    pub fn protocol(&self) -> ConnectionProtocol {
        self.record.connection.protocol()
    }
    pub fn channel(&self) -> ConnectionChannel {
        self.record.connection.channel()
    }
    pub fn is_unreachable(&self) -> bool {
        self.record.connection.is_unreachable()
    }
    pub fn close(&self) {
        self.record.connection.close();
    }
    pub async fn closed(&self) {
        self.record.connection.closed().await;
    }
    pub async fn check_current(&self) -> Result<(), ConnectivityError> {
        if let Some(request) = &self.request {
            request.check_current().await?;
        }
        self.record.grant.check_current().await
    }
    fn claim_exchange(&self) -> Result<(), ConnectivityError> {
        if self.mode() == ConnectionMode::SingleExchange
            && self.record.exchange_claimed.swap(true, Ordering::AcqRel)
        {
            return Err(ConnectivityError::ExchangeAlreadyUsed);
        }
        Ok(())
    }
    pub async fn open_bi(&self, deadline: Instant) -> Result<StreamLease, ConnectivityError> {
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        let global_waiting = self
            .record
            .waiting_global
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        let waiting = self
            .record
            .peer
            .waiting
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        let permit = timeout_at(deadline, self.record.peer.outbound.clone().acquire_owned())
            .await
            .map_err(ConnectivityError::TimedOut)?
            .map_err(ConnectivityError::ResourceClosed)?;
        drop(waiting);
        drop(global_waiting);
        self.claim_exchange()?;
        let result = timeout_at(deadline, async {
            self.check_current().await?;
            if Instant::now() >= deadline {
                return Err(ConnectivityError::DeadlineExpired);
            }
            self.record.connection.open_bi().await
        })
        .await
        .map_err(ConnectivityError::TimedOut)??;
        Ok(StreamLease::new(result, self.record.clone(), vec![permit]))
    }
    pub async fn accept_bi(&self) -> Result<StreamLease, ConnectivityError> {
        let local = self
            .record
            .inbound
            .clone()
            .acquire_owned()
            .await
            .map_err(ConnectivityError::ResourceClosed)?;
        self.claim_exchange()?;
        // 空闲等待仅占本连接额度；每连接至多一条已接收待排队流，不提前读负载。
        let pair = self.record.connection.accept_bi().await?;
        let global = self
            .record
            .inbound_global
            .clone()
            .acquire_owned()
            .await
            .map_err(ConnectivityError::ResourceClosed)?;
        self.check_current().await?;
        Ok(StreamLease::new(
            pair,
            self.record.clone(),
            vec![local, global],
        ))
    }
}
