use super::handle::Inner;
use super::ProgressProjectionRegistry;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// 只驱动投影状态变化通知，不发送网络或创建第二个进度交换循环。
pub struct ProgressProjectionWorker {
    pub(super) inner: Arc<Inner>,
}
impl ProgressProjectionWorker {
    pub async fn run(self, cancel: CancellationToken) {
        let registry = ProgressProjectionRegistry {
            inner: self.inner.clone(),
        };
        loop {
            if cancel.is_cancelled() {
                registry.close();
            }
            let (closed, active, keys) = {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                (
                    state.capacity.is_closed(),
                    state.registrations.active(),
                    state.slots.keys().copied().collect::<Vec<_>>(),
                )
            };
            if closed && active == 0 {
                break;
            }
            let mut has_deadline = false;
            for key in keys {
                if let Some(receiver) = registry.receiver(key) {
                    registry.publish(key, &receiver);
                    has_deadline |= receiver.needs_tick();
                }
            }
            tokio::select! {
                biased;
                _=cancel.cancelled(),if !closed=>{},
                _=self.inner.wake.notified()=>{},
                _=tokio::time::sleep(Duration::from_millis(100)),if has_deadline=>{},
            }
        }
    }
}
impl Drop for ProgressProjectionWorker {
    fn drop(&mut self) {
        ProgressProjectionRegistry {
            inner: self.inner.clone(),
        }
        .close();
    }
}
