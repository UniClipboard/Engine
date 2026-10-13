use super::{HistoryIncoming, HistoryReply, HistoryWork};
use crate::connectivity::{
    AdmissionGrant, ConnectionProtocol, ConnectivityError, PeerConnection, PeerReceiveStream,
    PeerSendStream, PendingInboundLease,
};
use crate::space::membership::MembershipLedgerError;
use async_trait::async_trait;
use std::sync::Arc;
use uc_core::ids::DeviceId;
use uc_core::membership::{MembershipHistoryExchangeError, MembershipHistoryMessage};

#[async_trait]
pub trait HistoryWirePort: Send + Sync {
    async fn read_request(
        &self,
        receive: &mut dyn PeerReceiveStream,
    ) -> Result<HistoryIncoming, MembershipHistoryExchangeError>;
    async fn write_request(
        &self,
        send: &mut dyn PeerSendStream,
        message: MembershipHistoryMessage,
    ) -> Result<(), MembershipHistoryExchangeError>;
    async fn read_response(
        &self,
        receive: &mut dyn PeerReceiveStream,
        protocol: ConnectionProtocol,
    ) -> Result<HistoryReply, MembershipHistoryExchangeError>;
    async fn respond(
        &self,
        send: &mut dyn PeerSendStream,
        response: Result<MembershipHistoryMessage, MembershipHistoryExchangeError>,
        protocol: ConnectionProtocol,
        reusable: bool,
    ) -> Result<(), MembershipHistoryExchangeError>;
}
#[async_trait]
pub trait HistoryPeerBindingPort: Send + Sync {
    async fn bind(
        &self,
        connection: Arc<dyn PeerConnection>,
        message: &MembershipHistoryMessage,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError>;
}
#[async_trait]
pub trait HistoryConnectionRetentionPort: Send + Sync {
    async fn may_retain(&self, peer: &DeviceId) -> Result<bool, MembershipLedgerError>;
}
#[async_trait]
pub trait HistoryLocalWorkPort: Send + Sync {
    async fn execute(
        &self,
        work: HistoryWork,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError>;
}
#[async_trait]
pub trait HistoryReceiverPort: Send + Sync {
    async fn receive(
        &self,
        pending: PendingInboundLease,
    ) -> Result<(), MembershipHistoryExchangeError>;
}
