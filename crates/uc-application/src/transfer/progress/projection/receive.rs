use super::super::{
    CurrentProgressTaskPort, ProgressBound, ProgressContactNotice, ProgressReceiveError,
    ProgressReceiverPort, ProgressTerminalNotice, ProgressUpdateNotice,
};
use super::handle::Inner;
use super::{ProgressProjectionKey, ProgressProjectionRegistry};
use async_trait::async_trait;
use std::sync::{Arc, PoisonError};
use uc_core::file_transfer::{ExchangePriority, ProgressIdentity};
use uc_core::ids::DeviceId;

struct Registration {
    inner: Arc<Inner>,
    peer: DeviceId,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .registrations
            .complete(&self.peer);
        self.inner.wake.notify_one();
    }
}
impl ProgressProjectionRegistry {
    fn registration(&self, peer: DeviceId) -> Result<Registration, ProgressReceiveError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.capacity.is_closed() {
            return Err(ProgressReceiveError::UnknownTask);
        }
        if state
            .registrations
            .enqueue(peer, (), ExchangePriority::Normal)
            .is_err()
        {
            return Err(ProgressReceiveError::Busy);
        }
        // 本机直接准入，不保留等待者；所有未获额度的候选在持锁期间原样撤下。
        if state.registrations.next_ready().is_none() {
            state.registrations.remove_queued(&peer, &());
            return Err(ProgressReceiveError::Busy);
        }
        Ok(Registration {
            inner: self.inner.clone(),
            peer,
        })
    }
}
#[async_trait]
impl ProgressReceiverPort for ProgressProjectionRegistry {
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let key = ProgressProjectionKey {
            peer: *peer,
            source,
        };
        let receiver = self
            .receiver(key)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        let _registration = self.registration(*peer)?;
        let bound = receiver
            .register(peer, generation, source, exchange)
            .await?;
        if !self.publish(key, &receiver) {
            return Err(ProgressReceiveError::UnknownTask);
        }
        Ok(bound)
    }
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError> {
        let key = ProgressProjectionKey {
            peer: *peer,
            source: notice.source,
        };
        let receiver = self
            .receiver(key)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        receiver.adopt_contact(peer, generation, notice).await?;
        if !self.publish(key, &receiver) {
            return Err(ProgressReceiveError::UnknownTask);
        }
        Ok(())
    }
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        let key = ProgressProjectionKey {
            peer: *peer,
            source: notice.source,
        };
        let receiver = self
            .receiver(key)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        receiver.adopt_update(peer, generation, notice).await?;
        if !self.publish(key, &receiver) {
            return Err(ProgressReceiveError::UnknownTask);
        }
        Ok(())
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        let key = ProgressProjectionKey {
            peer: *peer,
            source: notice.source,
        };
        let receiver = self
            .receiver(key)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        let adopted = receiver.adopt_terminal(peer, generation, notice).await?;
        if !self.publish(key, &receiver) {
            return Err(ProgressReceiveError::UnknownTask);
        }
        Ok(adopted)
    }
}
