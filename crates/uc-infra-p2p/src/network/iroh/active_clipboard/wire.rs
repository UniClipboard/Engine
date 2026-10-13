//! 只适配唯一 codec；不承担业务、准入或连接顺序。
use async_trait::async_trait;
use uc_application::deps::{
    ActiveClipboardTransportError, ActiveClipboardWirePort, ActivePullResponse, ActiveStateMessage,
    PeerReceiveStream, PeerSendStream,
};
use uc_core::clipboard::ActiveClipboardState;
use uc_sync_protocol::active_clipboard_state::{self as wire, ActiveClipboardWireMessage};

use uc_sync_protocol::active_clipboard_pull::{self as pull_wire, PullResponse};

pub struct ActiveClipboardWire;
#[async_trait]
impl ActiveClipboardWirePort for ActiveClipboardWire {
    async fn write_state(
        &self,
        mut stream: &mut dyn PeerSendStream,
        state: &ActiveClipboardState,
    ) -> Result<(), ActiveClipboardTransportError> {
        wire::write_frame(
            &mut stream,
            &ActiveClipboardWireMessage {
                snapshot_hash: state.snapshot_hash.clone(),
                entry_id: state.entry_id.as_ref().to_owned(),
                activated_at_ms: state.activated_at_ms,
                activated_by: state.activated_by.as_str().to_owned(),
            },
        )
        .await
        .map_err(|source| ActiveClipboardTransportError::Wire(anyhow::Error::new(source)))
    }
    async fn read_state(
        &self,
        mut stream: &mut dyn PeerReceiveStream,
    ) -> Result<ActiveStateMessage, ActiveClipboardTransportError> {
        let message = wire::read_frame(&mut stream)
            .await
            .map_err(|source| ActiveClipboardTransportError::Wire(anyhow::Error::new(source)))?;
        Ok(ActiveStateMessage {
            snapshot_hash: message.snapshot_hash,
            entry_id: message.entry_id,
            activated_at_ms: message.activated_at_ms,
            activated_by: message.activated_by,
        })
    }
    async fn write_pull_request(
        &self,
        mut stream: &mut dyn PeerSendStream,
        hash: &str,
    ) -> Result<(), ActiveClipboardTransportError> {
        pull_wire::write_request(&mut stream, hash)
            .await
            .map_err(|source| ActiveClipboardTransportError::Wire(anyhow::Error::new(source)))
    }
    async fn read_pull_request(
        &self,
        mut stream: &mut dyn PeerReceiveStream,
    ) -> Result<String, ActiveClipboardTransportError> {
        pull_wire::read_request(&mut stream)
            .await
            .map_err(|source| ActiveClipboardTransportError::Wire(anyhow::Error::new(source)))
    }
    async fn write_pull_response(
        &self,
        mut stream: &mut dyn PeerSendStream,
        response: ActivePullResponse,
    ) -> Result<(), ActiveClipboardTransportError> {
        let response = match response {
            ActivePullResponse::Envelope(bytes) => PullResponse::Envelope(bytes),
            ActivePullResponse::NotAvailable => PullResponse::NotAvailable,
            ActivePullResponse::Locked => PullResponse::Locked,
            ActivePullResponse::Internal => PullResponse::Internal,
        };
        pull_wire::write_response(&mut stream, &response)
            .await
            .map_err(|source| ActiveClipboardTransportError::Wire(anyhow::Error::new(source)))
    }
    async fn read_pull_response(
        &self,
        mut stream: &mut dyn PeerReceiveStream,
    ) -> Result<ActivePullResponse, ActiveClipboardTransportError> {
        let response = pull_wire::read_response(&mut stream)
            .await
            .map_err(|source| ActiveClipboardTransportError::Wire(anyhow::Error::new(source)))?;
        Ok(match response {
            PullResponse::Envelope(bytes) => ActivePullResponse::Envelope(bytes),
            PullResponse::NotAvailable => ActivePullResponse::NotAvailable,
            PullResponse::Locked => ActivePullResponse::Locked,
            PullResponse::Internal => ActivePullResponse::Internal,
        })
    }
}
