//! 方向名额同时覆盖建立、使用、本地结算与连接实际关闭。
use super::manager::PeerState;
use super::ConnectivityError;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{watch, OwnedSemaphorePermit};
use tokio_util::sync::CancellationToken;

struct DirectionSlot {
    _peer: Arc<PeerState>,
    _capacity: OwnedSemaphorePermit,
}
pub(super) struct ConnectionLifetime {
    unidentified: Mutex<Option<OwnedSemaphorePermit>>,
    direction: Mutex<Option<DirectionSlot>>,
    settled: watch::Sender<bool>,
}
impl Default for ConnectionLifetime {
    fn default() -> Self {
        Self {
            unidentified: Mutex::new(None),
            direction: Mutex::new(None),
            settled: watch::channel(false).0,
        }
    }
}
impl Drop for ConnectionLifetime {
    fn drop(&mut self) {
        // 先实际归还方向名额，再唤醒重连；接收者不持有本 lifetime。
        self.direction
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.settled.send_replace(true);
    }
}
impl ConnectionLifetime {
    pub fn settled(&self) -> watch::Receiver<bool> {
        self.settled.subscribe()
    }
    pub fn inbound(capacity: OwnedSemaphorePermit) -> Arc<Self> {
        Arc::new(Self {
            unidentified: Mutex::new(Some(capacity)),
            direction: Mutex::new(None),
            settled: watch::channel(false).0,
        })
    }
    pub async fn outbound(
        peer: Arc<PeerState>,
        cancel: &CancellationToken,
    ) -> Result<Arc<Self>, ConnectivityError> {
        let lifetime = Arc::new(Self::default());
        if let Err(source) = lifetime.reserve(peer.clone(), false) {
            if !matches!(source, ConnectivityError::Busy(_)) {
                return Err(source);
            }
            let pending = peer
                .retained_outbound_settlement
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let Some(mut settled) = pending else {
                return Err(source);
            };
            // 原保留连接已不能复用；新 flight 仍受原等待额度、取消与绝对期限约束。
            // 只有旧网络关闭和本地工作共同结算，原 lifetime 才归还方向名额并通知。
            // 首次取额失败后可能已经结算；wait_for 同样处理已完成通知，再取额一次。
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
                result = settled.wait_for(|finished| *finished) => {
                    result.map_err(ConnectivityError::ResultClosed)?;
                }
            }
            lifetime.reserve(peer, false)?;
        }
        Ok(lifetime)
    }
    pub fn reserve(&self, peer: Arc<PeerState>, inbound: bool) -> Result<(), ConnectivityError> {
        let pool = if inbound {
            &peer.inbound_connections
        } else {
            &peer.outbound_connections
        };
        if let Some(pool) = pool {
            let mut direction = self
                .direction
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if direction.is_some() {
                return Err(ConnectivityError::Denied);
            }
            let capacity = pool
                .clone()
                .try_acquire_owned()
                .map_err(ConnectivityError::Busy)?;
            *direction = Some(DirectionSlot {
                _peer: peer,
                _capacity: capacity,
            });
        }
        Ok(())
    }
    pub fn take_unidentified(&self) -> Option<OwnedSemaphorePermit> {
        self.unidentified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
