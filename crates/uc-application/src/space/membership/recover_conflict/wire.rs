//! 成员分支恢复的领域交换消息；wire版本与编码由Infra承担。
use async_trait::async_trait;
use tokio::time::Instant;
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1, MembershipConflictId,
};

use crate::connectivity::{PeerReceiveStream, PeerSendStream};

pub enum BranchRecoveryRequest {
    GroupInfo {
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
    },
    ExternalCommit {
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        external_commit: Vec<u8>,
    },
}

pub enum BranchRecoveryResponse {
    GroupInfo(Vec<u8>),
    RecoveryPackage(MembershipBranchRecoveryPackageV1),
    Rejected,
}

#[derive(Debug, thiserror::Error)]
pub enum BranchRecoveryWireError {
    #[error("branch recovery transport unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("branch recovery wire message invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
}

/// 一次交换共用调用方冻结的绝对期限，不负责拨号或业务授权。
#[async_trait]
pub trait BranchRecoveryWirePort: Send + Sync {
    async fn write_request(
        &self,
        send: &mut dyn PeerSendStream,
        request: BranchRecoveryRequest,
        deadline: Instant,
    ) -> Result<(), BranchRecoveryWireError>;

    async fn read_request(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<BranchRecoveryRequest, BranchRecoveryWireError>;

    async fn write_response(
        &self,
        send: &mut dyn PeerSendStream,
        response: BranchRecoveryResponse,
        deadline: Instant,
    ) -> Result<(), BranchRecoveryWireError>;

    async fn read_response(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<BranchRecoveryResponse, BranchRecoveryWireError>;
}
