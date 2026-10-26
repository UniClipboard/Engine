use super::lock;
use crate::{BindingError, BindingResourceRelease};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[derive(Default)]
pub(super) struct ShutdownWait {
    result: Mutex<Option<BindingResourceRelease>>,
    ready: Condvar,
}

impl ShutdownWait {
    pub(super) fn wait(&self, deadline: Instant) -> BindingResourceRelease {
        let mut current = lock(&self.result);
        loop {
            if let Some(result) = &*current {
                return result.clone();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return BindingResourceRelease::Pending;
            }
            current = self
                .ready
                .wait_timeout(current, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

/// 回应未完成即被丢弃时，用释放未确认的失败唤醒所有等待方。
pub(super) struct ShutdownReply(pub(super) Arc<ShutdownWait>);

impl ShutdownReply {
    pub(super) fn send(
        &self,
        result: BindingResourceRelease,
    ) -> Result<(), BindingResourceRelease> {
        let mut current = lock(&self.0.result);
        if current.is_some() {
            return Err(result);
        }
        *current = Some(result);
        self.0.ready.notify_all();
        Ok(())
    }
}

impl Drop for ShutdownReply {
    fn drop(&mut self) {
        let _ = self.send(BindingResourceRelease::Failed {
            error: BindingError::RuntimeUnavailable,
        });
    }
}
