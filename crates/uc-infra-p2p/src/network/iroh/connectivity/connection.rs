use super::super::conn_path::connection_channel;
use super::stream::wrap;
use async_trait::async_trait;
use iroh::endpoint::Connection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::broadcast;
use uc_application::deps::{ConnectionMode, ConnectivityError, PeerConnection, StreamPair};
use uc_core::ids::DeviceId;
use uc_core::ports::{ConnectionChannel, PeerReachabilityChanged, ReachabilityState};
use uc_sync_protocol::clipboard::MULTIPLEXED_CLIPBOARD_ALPN;

pub(crate) struct IrohPeerConnection {
    native: Connection,
    unreachable: AtomicBool,
    liveness: Option<(
        DeviceId,
        Mutex<broadcast::Receiver<PeerReachabilityChanged>>,
    )>,
}
impl IrohPeerConnection {
    pub(crate) fn wrap(connection: Connection) -> Arc<dyn PeerConnection> {
        Arc::new(Self {
            native: connection,
            unreachable: AtomicBool::new(false),
            liveness: None,
        })
    }
    fn observe_liveness(&self) {
        let Some((target, events)) = &self.liveness else {
            return;
        };
        let mut events = events.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match events.try_recv() {
                Ok(event)
                    if event.device_id == *target && event.state == ReachabilityState::Offline =>
                {
                    self.unreachable.store(true, Ordering::Release);
                    self.close();
                    return;
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.unreachable.store(true, Ordering::Release);
                    self.close();
                    return;
                }
                Err(_) => return,
            }
        }
    }
    pub(crate) fn wrap_outbound(
        connection: Connection,
        target: DeviceId,
        events: broadcast::Receiver<PeerReachabilityChanged>,
    ) -> Arc<dyn PeerConnection> {
        Arc::new(Self {
            native: connection,
            unreachable: AtomicBool::new(false),
            liveness: Some((target, Mutex::new(events))),
        })
    }
}
#[async_trait]
impl PeerConnection for IrohPeerConnection {
    fn id(&self) -> usize {
        self.native.stable_id()
    }
    fn authenticated_key(&self) -> [u8; 32] {
        *self.native.remote_id().as_bytes()
    }
    fn mode(&self) -> ConnectionMode {
        if self.native.alpn() == MULTIPLEXED_CLIPBOARD_ALPN {
            ConnectionMode::Multiplexed
        } else {
            ConnectionMode::SingleExchange
        }
    }
    fn channel(&self) -> ConnectionChannel {
        connection_channel(&self.native)
    }
    fn is_closed(&self) -> bool {
        // 获取租约同步消费已排队的离线证据，不依赖监督任务先获得调度。
        self.observe_liveness();
        self.native.close_reason().is_some()
    }
    fn is_unreachable(&self) -> bool {
        self.unreachable.load(Ordering::Acquire)
    }
    fn close(&self) {
        self.native.close(0u32.into(), b"clipboard exchange ended");
    }
    async fn closed(&self) {
        if let Some((target, events)) = &self.liveness {
            let mut events = events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .resubscribe();
            self.observe_liveness();
            loop {
                tokio::select! {
                    _ = self.native.closed() => return,
                    event = events.recv() => match event {
                        Ok(event) if event.device_id == *target && event.state == ReachabilityState::Offline => {
                            self.unreachable.store(true, Ordering::Release);
                            self.close();
                            return;
                        }
                        Ok(_) => self.observe_liveness(),
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // 丢失已确认离线的历史后，不让未知旧连接继续供新交换复用。
                            self.unreachable.store(true, Ordering::Release);
                            self.close();
                            return;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        let _ = self.native.closed().await;
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.observe_liveness();
        self.native
            .open_bi()
            .await
            .map(|(send, recv)| wrap(send, recv))
            .map_err(|error| ConnectivityError::Stream(Box::new(error)))
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.native
            .accept_bi()
            .await
            .map(|(send, recv)| wrap(send, recv))
            .map_err(|error| ConnectivityError::Stream(Box::new(error)))
    }
}
