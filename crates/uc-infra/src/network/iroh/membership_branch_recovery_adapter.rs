//! 认证 Iroh 连接上的成员分支恢复服务端。

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uc_application::deps::{
    BeginMembershipBranchRecoveryInput, IssueMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryPort, MembershipBranchRecoveryChannelError,
    MembershipBranchRecoveryChannelPort, MembershipBranchRecoveryCommit,
    MembershipBranchRecoveryRequest,
};
use uc_core::ids::DeviceId;
use uc_core::membership::MemberRepositoryPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use iroh::{Endpoint, EndpointAddr};

use super::connect_with_staggered_retry;
use super::membership_branch_recovery_wire::{
    decode, encode, MembershipBranchRecoveryWireMessage, MAX_RECOVERY_FRAME_SIZE,
};

const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub struct IrohMembershipBranchRecoveryChannel {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
}

impl IrohMembershipBranchRecoveryChannel {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
        }
    }

    async fn exchange(
        &self,
        peer_device_id: &DeviceId,
        request: MembershipBranchRecoveryWireMessage,
    ) -> Result<MembershipBranchRecoveryWireMessage, MembershipBranchRecoveryChannelError> {
        let address: EndpointAddr = self
            .peer_addr_repo
            .get(peer_device_id)
            .await
            .map_err(|source| unavailable(anyhow::Error::new(source)))?
            .and_then(|record| postcard::from_bytes(&record.addr_blob).ok())
            .ok_or_else(|| unavailable(anyhow::anyhow!("recovery peer address is unavailable")))?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            address,
            super::membership_branch_recovery_wire::MEMBERSHIP_BRANCH_RECOVERY_ALPN,
            "membership-branch-recovery",
        )
        .await
        .map_err(|source| unavailable(anyhow::Error::msg(source)))?;
        let (mut send, mut receive) = tokio::time::timeout(IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|source| unavailable(anyhow::Error::new(source)))?
            .map_err(|source| unavailable(anyhow::Error::new(source)))?;
        let bytes = encode(&request).map_err(|source| invalid(anyhow::Error::new(source)))?;
        let length =
            u32::try_from(bytes.len()).map_err(|source| invalid(anyhow::Error::new(source)))?;
        tokio::time::timeout(IO_TIMEOUT, async {
            send.write_u32(length).await?;
            send.write_all(&bytes).await?;
            send.finish()?;
            Ok::<(), std::io::Error>(())
        })
        .await
        .map_err(|source| unavailable(anyhow::Error::new(source)))?
        .map_err(|source| unavailable(anyhow::Error::new(source)))?;
        let response = read_response(&mut receive).await?;
        if matches!(
            response,
            MembershipBranchRecoveryWireMessage::Rejected { .. }
        ) {
            return Err(rejected(anyhow::anyhow!("recovery request was rejected")));
        }
        Ok(response)
    }
}

#[async_trait::async_trait]
impl MembershipBranchRecoveryChannelPort for IrohMembershipBranchRecoveryChannel {
    async fn request_membership_branch_group_info(
        &self,
        request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        match self
            .exchange(
                &request.peer_device_id,
                MembershipBranchRecoveryWireMessage::request_group_info(
                    request.conflict_id,
                    request.target_branch_id,
                    request.recipient_member,
                ),
            )
            .await?
        {
            MembershipBranchRecoveryWireMessage::GroupInfo { group_info, .. } => Ok(group_info),
            _ => Err(invalid(anyhow::anyhow!("unexpected recovery response"))),
        }
    }

    async fn submit_membership_branch_external_commit(
        &self,
        request: MembershipBranchRecoveryCommit,
    ) -> Result<
        uc_core::membership::MembershipBranchRecoveryPackageV1,
        MembershipBranchRecoveryChannelError,
    > {
        match self
            .exchange(
                &request.request.peer_device_id,
                MembershipBranchRecoveryWireMessage::submit_external_commit(
                    request.request.conflict_id,
                    request.request.target_branch_id,
                    request.request.recipient_member,
                    request.external_commit,
                ),
            )
            .await?
        {
            MembershipBranchRecoveryWireMessage::RecoveryPackage { package, .. } => Ok(package),
            _ => Err(invalid(anyhow::anyhow!("unexpected recovery response"))),
        }
    }
}

fn unavailable(source: anyhow::Error) -> MembershipBranchRecoveryChannelError {
    MembershipBranchRecoveryChannelError::Unavailable { source }
}

fn rejected(source: anyhow::Error) -> MembershipBranchRecoveryChannelError {
    MembershipBranchRecoveryChannelError::Rejected { source }
}

fn invalid(source: anyhow::Error) -> MembershipBranchRecoveryChannelError {
    MembershipBranchRecoveryChannelError::Invalid { source }
}

async fn read_response(
    receive: &mut iroh::endpoint::RecvStream,
) -> Result<MembershipBranchRecoveryWireMessage, MembershipBranchRecoveryChannelError> {
    let length = tokio::time::timeout(IO_TIMEOUT, receive.read_u32())
        .await
        .map_err(|source| unavailable(anyhow::Error::new(source)))?
        .map_err(|source| unavailable(anyhow::Error::new(source)))? as usize;
    if length == 0 || length > MAX_RECOVERY_FRAME_SIZE {
        return Err(invalid(anyhow::anyhow!(
            "recovery response size is invalid"
        )));
    }
    let bytes = tokio::time::timeout(IO_TIMEOUT, receive.read_to_end(length))
        .await
        .map_err(|source| unavailable(anyhow::Error::new(source)))?
        .map_err(|source| unavailable(anyhow::Error::new(source)))?;
    if bytes.len() != length {
        return Err(invalid(anyhow::anyhow!("recovery response is incomplete")));
    }
    decode(&bytes).map_err(|source| invalid(anyhow::Error::new(source)))
}

#[derive(Clone)]
pub(crate) struct IrohMembershipBranchRecoveryHandler {
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    endpoint: Arc<dyn IssueMembershipBranchRecoveryPort>,
}

impl IrohMembershipBranchRecoveryHandler {
    pub(crate) fn new(
        member_repo: Arc<dyn MemberRepositoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        endpoint: Arc<dyn IssueMembershipBranchRecoveryPort>,
    ) -> Self {
        Self {
            member_repo,
            fingerprint_factory,
            endpoint,
        }
    }

    async fn source_device(&self, public_key: &[u8; 32]) -> Option<DeviceId> {
        let fingerprint = self.fingerprint_factory.from_public_key(public_key).ok()?;
        self.member_repo
            .list()
            .await
            .ok()?
            .into_iter()
            .find(|member| member.identity_fingerprint == fingerprint)
            .map(|member| member.device_id)
    }

    async fn dispatch(
        &self,
        source_device_id: DeviceId,
        message: MembershipBranchRecoveryWireMessage,
    ) -> MembershipBranchRecoveryWireMessage {
        match message {
            MembershipBranchRecoveryWireMessage::RequestGroupInfo {
                conflict_id,
                target_branch_id,
                recipient_member,
                ..
            } => match self
                .endpoint
                .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
                    source_device_id,
                    conflict_id,
                    target_branch_id,
                    recipient_member,
                })
                .await
            {
                Ok(group_info) => MembershipBranchRecoveryWireMessage::group_info(group_info),
                Err(_) => MembershipBranchRecoveryWireMessage::rejected(),
            },
            MembershipBranchRecoveryWireMessage::SubmitExternalCommit {
                conflict_id,
                target_branch_id,
                recipient_member,
                external_commit,
                ..
            } => match self
                .endpoint
                .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
                    source_device_id,
                    conflict_id,
                    target_branch_id,
                    recipient_member,
                    external_commit,
                })
                .await
            {
                Ok(package) => MembershipBranchRecoveryWireMessage::recovery_package(package),
                Err(_) => MembershipBranchRecoveryWireMessage::rejected(),
            },
            _ => MembershipBranchRecoveryWireMessage::rejected(),
        }
    }
}

impl std::fmt::Debug for IrohMembershipBranchRecoveryHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohMembershipBranchRecoveryHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohMembershipBranchRecoveryHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) =
            match tokio::time::timeout(IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                _ => return Ok(()),
            };
        let response = match self.source_device(connection.remote_id().as_bytes()).await {
            Some(source_device_id) => match read_request(&mut receive).await {
                Some(message) => self.dispatch(source_device_id, message).await,
                None => MembershipBranchRecoveryWireMessage::rejected(),
            },
            None => MembershipBranchRecoveryWireMessage::rejected(),
        };
        write_response(&mut send, &response).await;
        Ok(())
    }
}

async fn read_request(
    receive: &mut iroh::endpoint::RecvStream,
) -> Option<MembershipBranchRecoveryWireMessage> {
    let length = tokio::time::timeout(IO_TIMEOUT, receive.read_u32())
        .await
        .ok()?
        .ok()? as usize;
    if length == 0 || length > MAX_RECOVERY_FRAME_SIZE {
        return None;
    }
    let bytes = tokio::time::timeout(IO_TIMEOUT, receive.read_to_end(length))
        .await
        .ok()?
        .ok()?;
    (bytes.len() == length)
        .then(|| decode(&bytes).ok())
        .flatten()
}

async fn write_response(
    send: &mut iroh::endpoint::SendStream,
    response: &MembershipBranchRecoveryWireMessage,
) {
    let Ok(bytes) = encode(response) else {
        return;
    };
    let Ok(length) = u32::try_from(bytes.len()) else {
        return;
    };
    if send.write_u32(length).await.is_ok() {
        let _ = send.write_all(&bytes).await;
        let _ = send.finish();
    }
}
