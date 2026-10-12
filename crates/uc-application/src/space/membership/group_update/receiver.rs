use async_trait::async_trait;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use super::{
    GroupUpdateApplyCoordinator, GroupUpdateLocalWorkPort, GroupUpdateReceiveOutcome,
    GroupUpdateReceiverPort, GroupUpdateWirePort,
};
use crate::connectivity::ConnectionLease;
use crate::space::membership::{AcquireSpaceWorkPermitPort, SpaceWorkMode};
use uc_core::membership::KeyEpochError;
use uc_observability_contract::diagnostics::scope_authenticated_inbound;

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

struct Receiver {
    apply: Arc<GroupUpdateApplyCoordinator>,
    wire: Arc<dyn GroupUpdateWirePort>,
    local_work: Arc<dyn GroupUpdateLocalWorkPort>,
    permit: Arc<dyn AcquireSpaceWorkPermitPort>,
}

/// 网络启动前创建，Application 组装后绑定完整接收 owner；未绑定时失败关闭。
pub struct GroupUpdateEndpoint(RwLock<Option<Arc<Receiver>>>);
impl GroupUpdateEndpoint {
    pub fn unbound() -> Arc<Self> {
        Arc::new(Self(RwLock::new(None)))
    }
    pub(crate) fn bind(
        &self,
        apply: Arc<GroupUpdateApplyCoordinator>,
        wire: Arc<dyn GroupUpdateWirePort>,
        local_work: Arc<dyn GroupUpdateLocalWorkPort>,
        permit: Arc<dyn AcquireSpaceWorkPermitPort>,
    ) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(Receiver {
            apply,
            wire,
            local_work,
            permit,
        }));
    }
}
#[async_trait]
impl GroupUpdateReceiverPort for GroupUpdateEndpoint {
    async fn receive(
        &self,
        connection: ConnectionLease,
    ) -> Result<GroupUpdateReceiveOutcome, anyhow::Error> {
        let receiver = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(receiver) = receiver else {
            connection.close();
            return Ok(GroupUpdateReceiveOutcome::Unavailable(
                KeyEpochError::SpaceNotReady,
            ));
        };
        let deadline = tokio::time::Instant::now() + EXCHANGE_TIMEOUT;
        let result = tokio::time::timeout_at(deadline, async {
            receiver.wire.is_supported(connection.protocol())?;
            connection.check_current().await?;
            let mut stream = connection.accept_bi().await?;
            let request = receiver.wire.read(&mut stream).await?;
            connection.check_current().await?;
            scope_authenticated_inbound(async {
                let apply = receiver.apply.clone();
                let permit = receiver.permit.clone();
                let applying_connection = connection.clone();
                let outcome = receiver
                    .local_work
                    .execute(Box::pin(request.observation.scope(async move {
                        let _permit = permit.acquire_space_work_permit().await?;
                        if _permit.mode() != SpaceWorkMode::Active {
                            return Ok(GroupUpdateReceiveOutcome::Unavailable(
                                KeyEpochError::SpaceNotReady,
                            ));
                        }
                        apply.receive(&applying_connection, &request.payload).await
                    })))
                    .await?;
                connection.check_current().await?;
                let acknowledged = receiver
                    .wire
                    .acknowledge(&mut stream, connection.protocol(), &outcome)
                    .await?;
                if !acknowledged {
                    connection.close();
                }
                Ok::<_, anyhow::Error>(outcome)
            })
            .await
        })
        .await;
        match result {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(source)) => {
                connection.close();
                Err(source)
            }
            Err(source) => {
                connection.close();
                Err(anyhow::Error::new(source))
            }
        }
    }
}
