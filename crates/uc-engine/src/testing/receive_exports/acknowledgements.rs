//! 在原owner真实采用之后、原生ACK编码之前暂停完整能力返回；不伪造采用或回执。
use super::{fixture_for, ReceiveExports};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use uc_application::deps::{
    CurrentProgressTaskPort, ProgressBound, ProgressContactNotice, ProgressReceiveError,
    ProgressReceiverPort, ProgressTerminalNotice, ProgressUpdateNotice,
};
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::DeviceId;

pub(crate) fn decorate_progress_receiver(
    path: &Path,
    inner: Arc<dyn ProgressReceiverPort>,
) -> Arc<dyn ProgressReceiverPort> {
    match fixture_for(path) {
        Some(fixture) => Arc::new(ObservedAcknowledgements { inner, fixture }),
        None => inner,
    }
}
struct ObservedAcknowledgements {
    inner: Arc<dyn ProgressReceiverPort>,
    fixture: Arc<ReceiveExports>,
}
#[async_trait]
impl ProgressReceiverPort for ObservedAcknowledgements {
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.inner.adopt_contact(peer, generation, notice).await
    }
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.inner.adopt_update(peer, generation, notice).await
    }
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        self.inner
            .register(peer, generation, source, exchange)
            .await
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        let adopted = self.inner.adopt_terminal(peer, generation, notice).await?;
        loop {
            let released = self.fixture.ack_release.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self.fixture.hold_ack_peer.lock().unwrap().as_ref() != Some(peer) {
                break;
            }
            self.fixture.ack_held.notify_one();
            released.await;
        }
        Ok(adopted)
    }
}
