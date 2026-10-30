//! 完整单次分支恢复交换，恢复步骤仍由成员业务负责人决定。
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::time::{timeout_at, Instant};
use uc_core::ids::DeviceId;
use uc_core::membership::MembershipBranchRecoveryPackageV1;

use super::{
    BranchRecoveryOperationScopePort, BranchRecoveryRequest, BranchRecoveryResponse,
    BranchRecoveryWireError, BranchRecoveryWirePort, MembershipBranchRecoveryChannelError,
    MembershipBranchRecoveryChannelPort, MembershipBranchRecoveryCommit,
    MembershipBranchRecoveryRequest,
};
use crate::connectivity::{
    ConnectionLease, ConnectionProtocol, ConnectionRequest, ConnectivityError, ConnectivityManager,
};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct BranchRecoveryTransport {
    manager: ConnectivityManager,
    wire: Arc<dyn BranchRecoveryWirePort>,
}

struct ManagedRecoveryScope {
    request: ConnectionRequest,
    wire: Arc<dyn BranchRecoveryWirePort>,
}

struct ExchangeLease(ConnectionLease);
impl Drop for ExchangeLease {
    fn drop(&mut self) {
        self.0.close();
    }
}

impl BranchRecoveryTransport {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn BranchRecoveryWirePort>) -> Self {
        Self { manager, wire }
    }
}

#[async_trait]
impl BranchRecoveryOperationScopePort for ManagedRecoveryScope {
    fn check_frozen(&self) -> Result<(), MembershipBranchRecoveryChannelError> {
        self.request.check_frozen().map_err(unavailable)
    }

    async fn exchange(
        &self,
        message: BranchRecoveryRequest,
    ) -> Result<BranchRecoveryResponse, MembershipBranchRecoveryChannelError> {
        let deadline = Instant::now() + EXCHANGE_TIMEOUT;
        let request = &self.request;
        let cancel = request.cancellation();
        let exchange = async {
            let connection = ExchangeLease(
                request
                    .connect(deadline, &cancel)
                    .await
                    .map_err(map_connectivity_error)?,
            );
            if connection.0.protocol() != ConnectionProtocol::MembershipBranchRecovery {
                return Err(MembershipBranchRecoveryChannelError::Rejected {
                    source: anyhow::anyhow!("unexpected recovery protocol"),
                });
            }
            let mut stream = connection
                .0
                .open_bi(deadline)
                .await
                .map_err(map_connectivity_error)?;
            connection
                .0
                .check_current()
                .await
                .map_err(map_connectivity_error)?;
            self.wire
                .write_request(stream.send.as_mut(), message, deadline)
                .await
                .map_err(map_wire_error)?;
            let response = self
                .wire
                .read_response(stream.receive.as_mut(), deadline)
                .await
                .map_err(map_wire_error)?;
            connection
                .0
                .check_current()
                .await
                .map_err(map_connectivity_error)?;
            match response {
                BranchRecoveryResponse::Rejected => {
                    Err(MembershipBranchRecoveryChannelError::Rejected {
                        source: anyhow::anyhow!("recovery request was rejected"),
                    })
                }
                response => Ok(response),
            }
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(MembershipBranchRecoveryChannelError::Unavailable { source: anyhow::anyhow!("recovery session stopped") }),
            result = timeout_at(deadline, exchange) => result.map_err(unavailable)?,
        };
        if request.check_frozen().is_err() {
            return Err(unavailable(anyhow::anyhow!(
                "recovery operation scope retired"
            )));
        }
        result
    }
}

fn unavailable(source: impl Into<anyhow::Error>) -> MembershipBranchRecoveryChannelError {
    MembershipBranchRecoveryChannelError::Unavailable {
        source: source.into(),
    }
}

fn map_connectivity_error(source: ConnectivityError) -> MembershipBranchRecoveryChannelError {
    fn denied(source: &ConnectivityError) -> bool {
        match source {
            ConnectivityError::Denied => true,
            ConnectivityError::Shared(inner) => denied(inner),
            _ => false,
        }
    }
    if denied(&source) {
        MembershipBranchRecoveryChannelError::Rejected {
            source: source.into(),
        }
    } else {
        unavailable(source)
    }
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for BranchRecoveryTransport {
    fn freeze(
        &self,
        peer: DeviceId,
    ) -> Result<Arc<dyn BranchRecoveryOperationScopePort>, MembershipBranchRecoveryChannelError>
    {
        let request = self
            .manager
            .freeze_request()
            .bind_peer(peer)
            .map_err(unavailable)?;
        Ok(Arc::new(ManagedRecoveryScope {
            request,
            wire: self.wire.clone(),
        }))
    }

    async fn request_membership_branch_group_info(
        &self,
        request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        match request
            .scope
            .exchange(BranchRecoveryRequest::GroupInfo {
                conflict_id: request.conflict_id,
                target_branch_id: request.target_branch_id,
                recipient_member: request.recipient_member,
            })
            .await?
        {
            BranchRecoveryResponse::GroupInfo(group_info) => Ok(group_info),
            _ => Err(MembershipBranchRecoveryChannelError::Invalid {
                source: anyhow::anyhow!("unexpected recovery response"),
            }),
        }
    }

    async fn submit_membership_branch_external_commit(
        &self,
        commit: MembershipBranchRecoveryCommit,
    ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError> {
        let request = commit.request;
        match request
            .scope
            .exchange(BranchRecoveryRequest::ExternalCommit {
                conflict_id: request.conflict_id,
                target_branch_id: request.target_branch_id,
                recipient_member: request.recipient_member,
                external_commit: commit.external_commit,
            })
            .await?
        {
            BranchRecoveryResponse::RecoveryPackage(package) => Ok(package),
            _ => Err(MembershipBranchRecoveryChannelError::Invalid {
                source: anyhow::anyhow!("unexpected recovery response"),
            }),
        }
    }
}

fn map_wire_error(error: BranchRecoveryWireError) -> MembershipBranchRecoveryChannelError {
    match error {
        BranchRecoveryWireError::Unavailable { .. } => {
            MembershipBranchRecoveryChannelError::Unavailable {
                source: error.into(),
            }
        }
        BranchRecoveryWireError::Invalid { .. } => MembershipBranchRecoveryChannelError::Invalid {
            source: error.into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{map_connectivity_error, ConnectivityError, MembershipBranchRecoveryChannelError};

    #[test]
    fn denied_and_shared_denied_are_rejected_without_losing_source() {
        for cause in [
            ConnectivityError::Denied,
            ConnectivityError::Shared(Arc::new(ConnectivityError::Denied)),
        ] {
            match map_connectivity_error(cause) {
                MembershipBranchRecoveryChannelError::Rejected { source } => {
                    assert!(source.downcast_ref::<ConnectivityError>().is_some());
                }
                _ => panic!("准入拒绝必须映射为拒绝"),
            }
        }
    }

    #[test]
    fn stopped_session_is_unavailable() {
        assert!(matches!(
            map_connectivity_error(ConnectivityError::Stopped),
            MembershipBranchRecoveryChannelError::Unavailable { .. }
        ));
    }
}
