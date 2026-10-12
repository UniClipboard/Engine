use async_trait::async_trait;
use futures::future::BoxFuture;
use uc_core::ids::DeviceId;
use uc_core::membership::{GroupEpoch, GroupUpdateDispatchError};

use super::{
    GroupUpdateApplyError, GroupUpdateIncoming, GroupUpdateReceiveOutcome, PreparedGroupUpdate,
    PreparedRemoval, VerifiedHistoryGroupUpdate,
};
use crate::connectivity::{ConnectionLease, ConnectionProtocol, ConnectivityError, StreamLease};
use crate::space::membership::{MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger};

#[async_trait]
pub(crate) trait RecoverGroupUpdatesPort: Send + Sync {
    async fn recover(
        &self,
        trigger: &MembershipMaintenanceTrigger,
        verified_contact: bool,
    ) -> MembershipMaintenanceStepOutcome;
}

pub type GroupUpdateWork = BoxFuture<'static, Result<GroupUpdateReceiveOutcome, anyhow::Error>>;

#[async_trait]
pub trait GroupUpdateReceiverPort: Send + Sync {
    async fn receive(
        &self,
        connection: ConnectionLease,
    ) -> Result<GroupUpdateReceiveOutcome, anyhow::Error>;
}

/// 已开始的本地工作必须持有到实际结束；网络等待方取消不能丢弃阻塞提交。
#[async_trait]
pub trait GroupUpdateLocalWorkPort: Send + Sync {
    async fn execute(
        &self,
        work: GroupUpdateWork,
    ) -> Result<GroupUpdateReceiveOutcome, anyhow::Error>;
}

#[async_trait]
pub trait ApplyGroupUpdatePort: Send + Sync {
    async fn prepare_removal(
        &self,
        _target: &DeviceId,
        _retained: &[DeviceId],
        _now_ms: i64,
    ) -> Result<Option<PreparedGroupUpdate>, anyhow::Error> {
        Err(anyhow::anyhow!(
            "group removal preparation is not available"
        ))
    }

    async fn apply(
        &self,
        payload: &[u8],
        proof: Option<&VerifiedHistoryGroupUpdate>,
        connection: Option<&ConnectionLease>,
    ) -> Result<GroupEpoch, GroupUpdateApplyError>;
}

#[async_trait]
pub trait GroupUpdateWirePort: Send + Sync {
    async fn read(&self, stream: &mut StreamLease) -> Result<GroupUpdateIncoming, anyhow::Error>;
    async fn acknowledge(
        &self,
        stream: &mut StreamLease,
        protocol: ConnectionProtocol,
        outcome: &GroupUpdateReceiveOutcome,
    ) -> Result<bool, anyhow::Error>;
    async fn send(
        &self,
        stream: &mut StreamLease,
        protocol: ConnectionProtocol,
        payload: &[u8],
    ) -> Result<(), GroupUpdateDispatchError>;
    fn is_supported(&self, protocol: ConnectionProtocol) -> Result<(), ConnectivityError>;
}

#[async_trait]
pub(crate) trait PrepareMembershipRemovalPort: Send + Sync {
    async fn prepare(
        &self,
        target: &DeviceId,
        retained: &[DeviceId],
        now_ms: i64,
    ) -> Result<PreparedRemoval, anyhow::Error>;
}

#[cfg(test)]
pub(crate) struct NoSecurityRemoval;
#[cfg(test)]
#[async_trait]
impl PrepareMembershipRemovalPort for NoSecurityRemoval {
    async fn prepare(
        &self,
        _: &DeviceId,
        _: &[DeviceId],
        _: i64,
    ) -> Result<PreparedRemoval, anyhow::Error> {
        Ok(PreparedRemoval {
            update: None,
            _guard: None,
        })
    }
}
