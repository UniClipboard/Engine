//! 方向名额同时覆盖建立、使用、本地结算与连接实际关闭。
use super::manager::PeerState;
use super::ConnectivityError;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{watch, OwnedSemaphorePermit};

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
    pub fn outbound(peer: Arc<PeerState>) -> Result<Arc<Self>, ConnectivityError> {
        let lifetime = Arc::new(Self::default());
        lifetime.reserve(peer, false)?;
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
