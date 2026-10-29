//! 单向流复用连接的准入、额度及工作寿命；停止流不停止共用连接。
use super::checks::checked;
use super::connection::ConnectionRecord;
use super::{
    ConnectionKey, ConnectionLease, ConnectivityError, PeerReceiveStream, PeerSendStream,
    StreamCapabilityRefusal, StreamWorkLease,
};
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

/// 单向流与完整工作使用权一起移交；底层工作可在网络停止后保留许可。
pub struct UniStreamLease<S, K: ConnectionKey = DeviceId> {
    pub stream: S,
    pub received_at: Option<Instant>,
    work: StreamWorkLease<K>,
}

impl<S, K: ConnectionKey> UniStreamLease<S, K> {
    fn new(
        stream: S,
        connection: Arc<ConnectionRecord<K>>,
        permits: Vec<OwnedSemaphorePermit>,
        received_at: Option<Instant>,
    ) -> Result<Self, ConnectivityError> {
        Ok(Self {
            stream,
            received_at,
            work: StreamWorkLease::new(connection, permits)?,
        })
    }

    pub fn into_parts(self) -> (S, StreamWorkLease<K>) {
        (self.stream, self.work)
    }
}

impl<K: ConnectionKey> UniStreamLease<Box<dyn PeerSendStream>, K> {
    pub fn stop(mut self) -> Result<(), ConnectivityError> {
        self.stream.reset(0)
    }
}

impl<K: ConnectionKey> UniStreamLease<Box<dyn PeerReceiveStream>, K> {
    pub fn stop(mut self) -> Result<(), ConnectivityError> {
        self.stream.stop(0)
    }
}

impl<K: ConnectionKey> ConnectionLease<K> {
    pub async fn open_uni(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<UniStreamLease<Box<dyn PeerSendStream>, K>, ConnectivityError> {
        let connection =
            self.record
                .connection
                .unidirectional()
                .ok_or(ConnectivityError::Capability(
                    StreamCapabilityRefusal::UnidirectionalUnavailable,
                ))?;
        let permit = self.reserve_outbound(deadline, cancel).await?;
        let stream = tokio::select! {
            biased;
            _ = self.record.stop.cancelled() => return Err(ConnectivityError::Stopped),
            _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
            result = timeout_at(deadline, connection.open_uni()) => result.map_err(ConnectivityError::TimedOut)??,
        };
        UniStreamLease::new(stream, self.record.clone(), vec![permit], None)
    }

    pub async fn accept_uni(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<UniStreamLease<Box<dyn PeerReceiveStream>, K>, ConnectivityError> {
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        let connection =
            self.record
                .connection
                .unidirectional()
                .ok_or(ConnectivityError::Capability(
                    StreamCapabilityRefusal::UnidirectionalUnavailable,
                ))?;
        // 等待对端开流不占工作额度；接到流后立即判容量，超量只停止该流。
        let stream = tokio::select! {
            biased;
            _ = self.record.stop.cancelled() => return Err(ConnectivityError::Stopped),
            _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
            result = timeout_at(deadline, connection.accept_uni()) => result.map_err(ConnectivityError::TimedOut)??,
        };
        let received_at = Instant::now();
        let mut lease =
            UniStreamLease::new(stream, self.record.clone(), Vec::new(), Some(received_at))?;
        let preparing = async {
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
            lease.work = StreamWorkLease::new(self.record.clone(), vec![local, global])?;
            self.claim_exchange()?;
            checked(
                self.check_current(),
                self.record.supervise_local_work,
                deadline,
                cancel,
            )
            .await?;
            if self.record.stop.is_cancelled() || cancel.is_cancelled() {
                return Err(ConnectivityError::Stopped);
            }
            if Instant::now() >= deadline {
                return Err(if self.record.supervise_local_work {
                    ConnectivityError::AdmissionDeadlineExpired
                } else {
                    ConnectivityError::DeadlineExpired
                });
            }
            Ok(())
        };
        if let Err(primary) = preparing.await {
            return match lease.stream.stop(0) {
                Ok(()) => Err(primary),
                Err(additional) => Err(ConnectivityError::Settlement {
                    primary: Box::new(primary),
                    additional: Box::new(additional),
                }),
            };
        }
        Ok(lease)
    }
}
