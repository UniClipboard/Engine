use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uc_application::deps::{
    ConnectionProtocol, ConnectivityError, GroupUpdateIncoming, GroupUpdateReceiveOutcome,
    GroupUpdateWirePort, StreamLease,
};
use uc_core::membership::{GroupUpdateDispatchError, GroupUpdateRejection};
use uc_observability_contract::diagnostics::{
    bind_authenticated_inbound_parent, capture_inbound_observation,
};
use uc_sync_protocol::group_update::{
    read_request, write_request, GroupUpdateAck, GroupUpdateVersion,
};

use super::super::trace_context::{authenticated_parent, inject_current};

pub struct GroupUpdateWire;
fn version(protocol: ConnectionProtocol) -> Result<GroupUpdateVersion, ConnectivityError> {
    match protocol {
        ConnectionProtocol::GroupUpdate => Ok(GroupUpdateVersion::Current),
        ConnectionProtocol::LegacyGroupUpdate => Ok(GroupUpdateVersion::Legacy),
        ConnectionProtocol::ActiveClipboardState
        | ConnectionProtocol::ActiveClipboardPull
        | ConnectionProtocol::Clipboard
        | ConnectionProtocol::AttributedClipboard
        | ConnectionProtocol::MultiplexedClipboard
        | ConnectionProtocol::MembershipHistory
        | ConnectionProtocol::MembershipBranchRecovery
        | ConnectionProtocol::MultiplexedMembershipHistory
        | ConnectionProtocol::PeerReachabilityV1
        | ConnectionProtocol::PeerReachabilityV2
        | ConnectionProtocol::Blob
        | ConnectionProtocol::TransferProgressV1
        | ConnectionProtocol::TransferProgressV2
        | ConnectionProtocol::SpaceAdmission
        | ConnectionProtocol::Unsupported => Err(ConnectivityError::Denied),
    }
}
#[async_trait]
impl GroupUpdateWirePort for GroupUpdateWire {
    fn is_supported(&self, protocol: ConnectionProtocol) -> Result<(), ConnectivityError> {
        version(protocol)?;
        Ok(())
    }
    async fn read(&self, stream: &mut StreamLease) -> Result<GroupUpdateIncoming, anyhow::Error> {
        let request = read_request(&mut stream.receive).await?;
        // 本方法只在完整接收 owner 已核对当前认证成员后调用。
        if let Some(parent) = authenticated_parent(request.trace_context.as_ref()) {
            let _ = bind_authenticated_inbound_parent(parent);
        }
        Ok(GroupUpdateIncoming {
            payload: request.payload,
            observation: capture_inbound_observation(),
        })
    }
    async fn acknowledge(
        &self,
        stream: &mut StreamLease,
        protocol: ConnectionProtocol,
        outcome: &GroupUpdateReceiveOutcome,
    ) -> Result<bool, anyhow::Error> {
        let ack = match outcome {
            GroupUpdateReceiveOutcome::Applied => GroupUpdateAck::Applied,
            GroupUpdateReceiveOutcome::Rejected(_) => GroupUpdateAck::Rejected,
            GroupUpdateReceiveOutcome::Unavailable(_) => GroupUpdateAck::Unavailable,
        };
        let Some(byte) = ack.encode(version(protocol)?) else {
            return Ok(false);
        };
        stream.send.write_all(&[byte]).await?;
        stream.send.finish()?;
        Ok(true)
    }
    async fn send(
        &self,
        stream: &mut StreamLease,
        protocol: ConnectionProtocol,
        payload: &[u8],
    ) -> Result<(), GroupUpdateDispatchError> {
        let version = version(protocol).map_err(GroupUpdateDispatchError::transport_from)?;
        write_request(&mut stream.send, payload, inject_current())
            .await
            .map_err(GroupUpdateDispatchError::transport_from)?;
        stream
            .send
            .finish()
            .map_err(GroupUpdateDispatchError::transport_from)?;
        let mut byte = [0];
        stream
            .receive
            .read_exact(&mut byte)
            .await
            .map_err(GroupUpdateDispatchError::transport_from)?;
        match GroupUpdateAck::decode(byte[0], version)
            .map_err(GroupUpdateDispatchError::transport_from)?
        {
            GroupUpdateAck::Applied => Ok(()),
            GroupUpdateAck::Unavailable => Err(GroupUpdateDispatchError::ReceiverUnavailable),
            GroupUpdateAck::Rejected => Err(GroupUpdateDispatchError::Rejected(match version {
                GroupUpdateVersion::Current => GroupUpdateRejection::ByReceiver,
                GroupUpdateVersion::Legacy => GroupUpdateRejection::Unclassified,
            })),
        }
    }
}
