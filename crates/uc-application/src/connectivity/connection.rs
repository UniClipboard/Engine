use super::checks::checked;
use super::lifetime::ConnectionLifetime;
use super::manager::PeerState;
use super::retention::{ActivityGuard, ConnectionActivity};
use super::{
    AdmissionAttempt, AdmissionGrant, ConnectionMode, ConnectionProtocol, ConnectivityError,
    PeerConnection, StreamLease, StreamPair,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;

pub(super) struct ConnectionRecord {
    pub connection: Arc<dyn PeerConnection>,
    pub address_hint: Option<Vec<u8>>,
    pub activity: Arc<ConnectionActivity>,
    pub stop: CancellationToken,
    pub cache_cancel: CancellationToken,
    pub supervise_local_work: bool,
    pub accepted_stream_timeout: Option<Duration>,
    pub reject_excess_inbound: bool,
    pub first_inbound_at: StdMutex<Option<Instant>>,
    pub pending: Mutex<Option<AcceptedStream>>,
    pub exchange_claimed: AtomicBool,
    pub grant: Box<dyn AdmissionGrant>,
    pub peer: Arc<PeerState>,
    pub inbound: Arc<Semaphore>,
    pub inbound_global: Arc<Semaphore>,
    pub waiting_global: Arc<Semaphore>,
    pub inbound_direction: bool,
    pub _lifetime: Arc<ConnectionLifetime>,
    pub _inbound_connection: Option<tokio::sync::OwnedSemaphorePermit>,
}

pub(super) struct AcceptedStream {
    pub pair: StreamPair,
    pub activity: Arc<ActivityGuard>,
    pub accepted_at: Instant,
}

impl Drop for ConnectionRecord {
    fn drop(&mut self) {
        self.connection.close();
    }
}

/// 连接使用权；出站请求活动随最后一份使用权释放，入站监听本身不算活动。
#[derive(Clone)]
pub struct ConnectionLease {
    pub(super) record: Arc<ConnectionRecord>,
    pub(super) request: Option<Arc<dyn AdmissionAttempt>>,
    pub(super) _activity: Option<Arc<ActivityGuard>>,
}

impl ConnectionLease {
    pub(super) fn outbound(
        record: Arc<ConnectionRecord>,
        request: Arc<dyn AdmissionAttempt>,
    ) -> Result<Self, ConnectivityError> {
        let activity = record.activity.acquire()?;
        Ok(Self {
            record,
            request: Some(request),
            _activity: Some(activity),
        })
    }
    pub fn connection_id(&self) -> usize {
        self.record.connection.id()
    }
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
    pub fn is_closed(&self) -> bool {
        self.record.connection.is_closed()
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
        if self.record.stop.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if let Some(request) = &self.request {
            request.check_current().await?;
        }
        self.record.grant.check_current().await
    }
    pub(super) fn claim_exchange(&self) -> Result<(), ConnectivityError> {
        if matches!(
            self.mode(),
            ConnectionMode::SingleExchange | ConnectionMode::RetainedSingleExchange
        ) && self.record.exchange_claimed.swap(true, Ordering::AcqRel)
        {
            return Err(ConnectivityError::ExchangeAlreadyUsed);
        }
        Ok(())
    }
    pub async fn open_bi(&self, deadline: Instant) -> Result<StreamLease, ConnectivityError> {
        self.open_bi_with_cancel(deadline, &CancellationToken::new())
            .await
    }
    /// 按协议策略停止网络步骤；受监督的 grant 读取持许可到实际结算。
    pub async fn open_bi_with_cancel(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<StreamLease, ConnectivityError> {
        let permit = self.reserve_outbound(deadline, cancel).await?;
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
            result = timeout_at(deadline, self.record.connection.open_bi()) => result.map_err(ConnectivityError::TimedOut)??,
        };
        StreamLease::new(result, self.record.clone(), vec![permit])
    }

    pub(super) async fn reserve_outbound(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, ConnectivityError> {
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
        let permit = tokio::select! {
            biased;
            _ = self.record.stop.cancelled() => return Err(ConnectivityError::Stopped),
            _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
            result = timeout_at(deadline, self.record.peer.outbound.clone().acquire_owned()) => result.map_err(ConnectivityError::TimedOut)?.map_err(ConnectivityError::ResourceClosed)?,
        };
        drop(waiting);
        drop(global_waiting);
        self.claim_exchange()?;
        checked(
            self.check_current(),
            self.record.supervise_local_work,
            deadline,
            cancel,
        )
        .await?;
        if Instant::now() >= deadline || self.record.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(if self.record.supervise_local_work {
                ConnectivityError::AdmissionDeadlineExpired
            } else {
                ConnectivityError::DeadlineExpired
            });
        }
        Ok(permit)
    }
    pub async fn accept_bi(&self) -> Result<StreamLease, ConnectivityError> {
        if self.protocol() == ConnectionProtocol::PeerReachabilityV2 {
            // 先取出请求，再立即判每连接容量；超量仅丢弃当前半流。
            let pair = self.record.connection.accept_bi().await?;
            let capacity = self
                .record
                .inbound
                .clone()
                .try_acquire_owned()
                .map_err(ConnectivityError::Busy)?;
            return StreamLease::new(pair, self.record.clone(), vec![capacity]);
        }
        if self.record.reject_excess_inbound {
            let first = *self
                .record
                .first_inbound_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let buffered = self.record.pending.lock().await.take();
            let mut held_activity = None;
            let pair = match buffered {
                Some(accepted) => {
                    held_activity = Some(accepted.activity);
                    accepted.pair
                }
                None => {
                    let accepting = self.record.connection.accept_bi();
                    match first.zip(self.record.accepted_stream_timeout) {
                        Some((start, timeout)) => timeout_at(start + timeout, accepting)
                            .await
                            .map_err(ConnectivityError::TimedOut)??,
                        None => accepting.await?,
                    }
                }
            };
            let started = self
                .record
                .first_inbound_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .unwrap_or_else(Instant::now);
            let mut stream = StreamLease::new(pair, self.record.clone(), Vec::new())?;
            stream.received_at = Some(started);
            drop(held_activity);
            let capacity = (|| {
                let local = self
                    .record
                    .inbound
                    .clone()
                    .try_acquire_owned()
                    .map_err(ConnectivityError::Busy)?;
                let global = self
                    .record
                    .inbound_global
                    .clone()
                    .try_acquire_owned()
                    .map_err(ConnectivityError::Busy)?;
                Ok(vec![local, global])
            })();
            let permits = match capacity {
                Ok(permits) => permits,
                Err(primary) => {
                    return match stream.stop() {
                        Ok(()) => Err(primary),
                        Err(additional) => Err(ConnectivityError::Settlement {
                            primary: Box::new(primary),
                            additional: Box::new(additional),
                        }),
                    }
                }
            };
            stream.attach_capacity(permits);
            self.claim_exchange()?;
            let deadline = started
                + self
                    .record
                    .accepted_stream_timeout
                    .ok_or(ConnectivityError::DeadlineExpired)?;
            if let Err(primary) = checked(
                self.check_current(),
                self.record.supervise_local_work,
                deadline,
                &self.record.stop,
            )
            .await
            {
                return match stream.stop() {
                    Ok(()) => Err(primary),
                    Err(additional) => Err(ConnectivityError::Settlement {
                        primary: Box::new(primary),
                        additional: Box::new(additional),
                    }),
                };
            }
            return Ok(stream);
        }
        let local = self
            .record
            .inbound
            .clone()
            .acquire_owned()
            .await
            .map_err(ConnectivityError::ResourceClosed)?;
        self.claim_exchange()?;
        // 已从 transport 取出的流保存在 record，select 取消排队时不丢弃另一请求。
        let mut pending = self.record.pending.lock().await;
        if pending.is_none() {
            let pair = self.record.connection.accept_bi().await?;
            let activity = self.record.activity.acquire()?;
            *pending = Some(AcceptedStream {
                pair,
                activity,
                accepted_at: Instant::now(),
            });
        }
        let accepted_at = pending
            .as_ref()
            .ok_or(ConnectivityError::Stopped)?
            .accepted_at;
        if self
            .record
            .accepted_stream_timeout
            .is_some_and(|timeout| Instant::now() >= accepted_at + timeout)
        {
            pending.take();
            return Err(ConnectivityError::DeadlineExpired);
        }
        let acquiring = self.record.inbound_global.clone().acquire_owned();
        let acquired = match self.record.accepted_stream_timeout {
            Some(timeout) => match timeout_at(accepted_at + timeout, acquiring).await {
                Ok(result) => result,
                Err(source) => {
                    pending.take();
                    return Err(ConnectivityError::TimedOut(source));
                }
            },
            None => acquiring.await,
        };
        let global = match acquired {
            Ok(permit) => permit,
            Err(source) => {
                pending.take();
                return Err(ConnectivityError::ResourceClosed(source));
            }
        };
        if let Err(source) = self.check_current().await {
            pending.take();
            return Err(source);
        }
        let accepted = pending.take().ok_or(ConnectivityError::Stopped)?;
        let mut stream = StreamLease::new(accepted.pair, self.record.clone(), vec![local, global])?;
        stream.received_at = Some(accepted.accepted_at);
        drop(accepted.activity);
        Ok(stream)
    }
}
