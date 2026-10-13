use super::error::ActiveClipboardTransportRefusal;
use super::model::CloseOnDrop;
use super::{ActiveClipboardTransportError, ActiveClipboardWirePort, ActiveStateReceiverPort};
use crate::connectivity::{ConnectivityManager, PeerConnection};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::time::{timeout_at, Instant};
use uc_core::ids::DeviceId;
use uc_core::ports::{ActiveClipboardReceiverPort, InboundActiveClipboardState};
use uc_observability_contract::{log_fields::log_vocab_debug, uc_info};

pub struct ActiveStateReceiver {
    manager: ConnectivityManager,
    wire: Arc<dyn ActiveClipboardWirePort>,
    events: broadcast::Sender<InboundActiveClipboardState>,
}
impl ActiveStateReceiver {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn ActiveClipboardWirePort>) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            manager,
            wire,
            events,
        }
    }
}
#[async_trait]
impl ActiveClipboardReceiverPort for ActiveStateReceiver {
    fn subscribe(&self) -> broadcast::Receiver<InboundActiveClipboardState> {
        self.events.subscribe()
    }
}
#[async_trait]
impl ActiveStateReceiverPort for ActiveStateReceiver {
    async fn receive(
        &self,
        connection: Arc<dyn PeerConnection>,
        deadline: Instant,
    ) -> Result<(), ActiveClipboardTransportError> {
        let _close = CloseOnDrop(connection.clone());
        timeout_at(deadline, async {
            let (connection, mut stream) = self.manager.accept_exchange(connection, deadline).await?;
            let message = self.wire.read_state(stream.receive.as_mut()).await?;
            let activated_by = DeviceId::try_new(&message.activated_by)
                .ok_or(ActiveClipboardTransportRefusal::InvalidActivator)?;
            connection.check_current().await?;
            let channel = connection.channel();
            let inbound = InboundActiveClipboardState {
                peer_device_id: connection.peer(), snapshot_hash: message.snapshot_hash,
                sender_entry_id: message.entry_id, activated_at_ms: message.activated_at_ms, activated_by,
            };
            // discarded-source[channel]: 通道错误携带敏感观察，不延长其生命周期。
            self.events.send(inbound).map_err(|_| ActiveClipboardTransportRefusal::NoSubscriber)?;
            uc_info!(operation = "active_state_receive", outcome = "handed_off", channel = log_vocab_debug(&channel), "active clipboard notification handed to local subscriber; application is unconfirmed");
            Ok::<_, ActiveClipboardTransportError>(())
        }).await?
    }
    async fn shutdown(&self) -> Result<(), ActiveClipboardTransportError> {
        self.manager.shutdown().await?;
        Ok(())
    }
}
