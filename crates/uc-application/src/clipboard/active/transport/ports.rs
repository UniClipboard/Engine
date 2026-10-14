use super::{
    ActiveClipboardTransportError, ActivePullPreparationError, ActivePullPreparationStop,
    ActivePullResponse, ActiveStateMessage,
};
use crate::connectivity::{PeerConnection, PeerReceiveStream, PeerSendStream};
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::time::Instant;
use uc_core::clipboard::ActiveClipboardState;

#[async_trait]
pub trait ActiveClipboardWirePort: Send + Sync {
    async fn write_state(
        &self,
        stream: &mut dyn PeerSendStream,
        state: &ActiveClipboardState,
    ) -> Result<(), ActiveClipboardTransportError>;
    async fn read_state(
        &self,
        stream: &mut dyn PeerReceiveStream,
    ) -> Result<ActiveStateMessage, ActiveClipboardTransportError>;
    async fn write_pull_request(
        &self,
        stream: &mut dyn PeerSendStream,
        hash: &str,
    ) -> Result<(), ActiveClipboardTransportError>;
    async fn read_pull_request(
        &self,
        stream: &mut dyn PeerReceiveStream,
    ) -> Result<String, ActiveClipboardTransportError>;
    async fn write_pull_response(
        &self,
        stream: &mut dyn PeerSendStream,
        response: ActivePullResponse,
    ) -> Result<(), ActiveClipboardTransportError>;
    async fn read_pull_response(
        &self,
        stream: &mut dyn PeerReceiveStream,
    ) -> Result<ActivePullResponse, ActiveClipboardTransportError>;
}

#[async_trait]
pub trait ActiveStateReceiverPort: Send + Sync {
    async fn receive(
        &self,
        connection: Arc<dyn PeerConnection>,
        deadline: Instant,
    ) -> Result<(), ActiveClipboardTransportError>;
    async fn shutdown(&self) -> Result<(), ActiveClipboardTransportError>;
}

#[async_trait]
pub trait ActivePullReceiverPort: Send + Sync {
    async fn receive(
        &self,
        connection: Arc<dyn PeerConnection>,
        deadline: Instant,
    ) -> Result<(), ActiveClipboardTransportError>;
    async fn shutdown(&self) -> Result<(), ActiveClipboardTransportError>;
}

pub type ActivePullWork =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, ActivePullPreparationError>> + Send + 'static>>;
#[async_trait]
pub trait ActivePullLocalWorkPort: Send + Sync {
    async fn execute(
        &self,
        work: ActivePullWork,
        stop: ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError>;
}
