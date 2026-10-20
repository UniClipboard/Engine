//! 共同拨号与旧模式独占拨号使用同一连接准入实现。
use super::connection::ConnectionRecord;
use super::manager::PeerState;
use super::retention::ConnectionActivity;
use super::{
    AdmissionAttempt, ConnectivityBackend, ConnectivityError, ConnectivityPolicy, PeerConnection,
};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

pub(super) struct Acquisition {
    pub backend: Arc<dyn ConnectivityBackend>,
    pub policy: ConnectivityPolicy,
    pub inbound: Arc<Semaphore>,
    pub waiting: Arc<Semaphore>,
    pub stop: CancellationToken,
    pub cancel: CancellationToken,
    pub pending: Arc<Mutex<Option<Arc<dyn PeerConnection>>>>,
}
struct PendingConnection {
    connection: Arc<dyn PeerConnection>,
    adopted: bool,
}
impl Drop for PendingConnection {
    fn drop(&mut self) {
        if !self.adopted {
            self.connection.close();
        }
    }
}
impl Acquisition {
    pub async fn connect(
        &self,
        target: DeviceId,
        peer: Arc<PeerState>,
        attempt: Arc<dyn AdmissionAttempt>,
    ) -> Result<Arc<ConnectionRecord>, ConnectivityError> {
        // 只在任何负载写出前更新一次已确认失活的连接；仍由原 flight/deadline 约束。
        for refresh in 0..2 {
            let record = self
                .connect_once(target, peer.clone(), attempt.clone())
                .await?;
            if !record.connection.is_closed() {
                return Ok(record);
            }
            if refresh == 1 || !record.connection.is_unreachable() {
                return Err(ConnectivityError::Offline);
            }
        }
        Err(ConnectivityError::Offline)
    }
    async fn connect_once(
        &self,
        target: DeviceId,
        peer: Arc<PeerState>,
        attempt: Arc<dyn AdmissionAttempt>,
    ) -> Result<Arc<ConnectionRecord>, ConnectivityError> {
        let mut pending = PendingConnection {
            connection: self.backend.connect(target, self.cancel.clone()).await?,
            adopted: false,
        };
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(pending.connection.clone());
        if self.cancel.is_cancelled() {
            return Err(super::ConnectivityError::Stopped);
        }
        let grant = attempt.adopt(pending.connection.clone()).await?;
        if self.cancel.is_cancelled() {
            return Err(super::ConnectivityError::Stopped);
        }
        // 准入失败、取消或任务 panic 都关闭未交给 record 的连接，与 backend 自身引用无关。
        pending.adopted = true;
        let connection = pending.connection.clone();
        Ok(Arc::new(ConnectionRecord {
            connection,
            activity: Arc::new(ConnectionActivity::default()),
            stop: self.stop.clone(),
            cache_cancel: CancellationToken::new(),
            presence_retention: self.policy.presence_retention,
            accepted_stream_timeout: self.policy.accepted_stream_timeout,
            pending: tokio::sync::Mutex::new(None),
            exchange_claimed: AtomicBool::new(false),
            grant,
            peer,
            inbound: Arc::new(Semaphore::new(self.policy.inbound_per_connection)),
            inbound_global: self.inbound.clone(),
            waiting_global: self.waiting.clone(),
            _inbound_connection: None,
        }))
    }
}
