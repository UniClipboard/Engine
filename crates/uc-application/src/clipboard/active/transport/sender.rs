use super::{
    ActiveClipboardTransportError, ActiveClipboardWirePort, ACTIVE_EXCHANGE_TIMEOUT,
    CLOSE_BARRIER_TIMEOUT,
};
use crate::connectivity::{ConnectivityError, ConnectivityManager};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uc_core::clipboard::ActiveClipboardState;
use uc_core::ids::DeviceId;
use uc_core::ports::{ActiveClipboardDispatchError, ActiveClipboardDispatchPort};

pub struct ActiveStateSender {
    manager: ConnectivityManager,
    wire: Arc<dyn ActiveClipboardWirePort>,
}
impl ActiveStateSender {
    pub fn new(manager: ConnectivityManager, wire: Arc<dyn ActiveClipboardWirePort>) -> Self {
        Self { manager, wire }
    }
}
#[async_trait]
impl ActiveClipboardDispatchPort for ActiveStateSender {
    #[instrument(name = "active_state.dispatch", skip_all)]
    async fn dispatch(
        &self,
        target: &DeviceId,
        state: &ActiveClipboardState,
    ) -> Result<(), ActiveClipboardDispatchError> {
        let deadline = Instant::now() + ACTIVE_EXCHANGE_TIMEOUT;
        timeout_at(deadline, async {
            let connection = self
                .manager
                .request(*target)
                .connect(deadline, &CancellationToken::new())
                .await?;
            let mut stream = connection.open_bi(deadline).await?;
            self.wire.write_state(stream.send.as_mut(), state).await?;
            connection.check_current().await?;
            stream.send.finish()?;
            // 原协议无 ACK；保护期结束只表示本机发送已结算。
            let barrier = Instant::now() + CLOSE_BARRIER_TIMEOUT;
            tokio::select! {
                _ = connection.closed() => {},
                _ = tokio::time::sleep_until(barrier.min(deadline)) => {
                    if Instant::now() >= deadline {
                        return Err(ActiveClipboardTransportError::Connectivity(
                            ConnectivityError::DeadlineExpired));
                    }
                }
            }
            Ok::<_, ActiveClipboardTransportError>(())
        })
        .await
        .map_err(ActiveClipboardTransportError::from)?
        .map_err(ActiveClipboardDispatchError::from)
    }
}
