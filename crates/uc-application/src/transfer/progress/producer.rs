//! 从已接受接收意图登记当前任务；所有网络步骤共用原期限，绑定返回后再次核对当前权威。
use super::{
    ProgressBound, ProgressReceiveError, ProgressRegistrationWirePort, ProgressTaskAuthorityPort,
};
use crate::connectivity::{ConnectionProtocol, ConnectivityError, ConnectivityManager};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::DeviceId;

pub struct TransferProgressProducer {
    manager: ConnectivityManager,
    wire: Arc<dyn ProgressRegistrationWirePort>,
    authority: Arc<dyn ProgressTaskAuthorityPort>,
}
impl TransferProgressProducer {
    pub fn new(
        manager: ConnectivityManager,
        wire: Arc<dyn ProgressRegistrationWirePort>,
        authority: Arc<dyn ProgressTaskAuthorityPort>,
    ) -> Self {
        Self {
            manager,
            wire,
            authority,
        }
    }
    pub async fn register_current(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        if self.authority.current_task(peer, source).is_none() {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let connection = self
            .manager
            .request(*peer)
            .connect(deadline, cancel)
            .await?;
        match connection.protocol() {
            ConnectionProtocol::TransferProgressV2 => {}
            ConnectionProtocol::TransferProgressV1 => {
                return Err(ProgressReceiveError::Unsupported)
            }
            _ => return Err(ProgressReceiveError::Protocol),
        }
        let mut stream = connection.open_bi_with_cancel(deadline, cancel).await?;
        let bound = self
            .wire
            .register(
                &mut stream,
                &connection,
                source,
                self.authority.as_ref(),
                deadline,
                cancel,
            )
            .await?;
        self.manager.retain(&connection).await?;
        if cancel.is_cancelled() {
            return Err(ProgressReceiveError::Connection(ConnectivityError::Stopped));
        }
        let current = self.authority.current_task(peer, source);
        if Instant::now() >= deadline {
            return Err(ProgressReceiveError::Connection(
                ConnectivityError::DeadlineExpired,
            ));
        }
        if cancel.is_cancelled() {
            return Err(ProgressReceiveError::Connection(ConnectivityError::Stopped));
        }
        if bound.descriptor.source != source
            || current.is_none_or(|current| current.descriptor != bound.descriptor)
        {
            return Err(ProgressReceiveError::Stale);
        }
        Ok(bound)
    }
}
