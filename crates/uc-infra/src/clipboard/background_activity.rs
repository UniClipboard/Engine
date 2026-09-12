use tokio::sync::{Notify, RwLock, RwLockReadGuard};

/// 一次完整磁盘动作持有读许可；暂停等待已有动作结束并阻止后续动作。
pub(super) struct BackgroundActivity {
    active: RwLock<bool>,
    changed: Notify,
}

impl BackgroundActivity {
    pub(super) fn new() -> Self {
        Self {
            active: RwLock::new(true),
            changed: Notify::new(),
        }
    }

    pub(super) async fn enter(&self) -> RwLockReadGuard<'_, bool> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let active = self.active.read().await;
            if *active {
                return active;
            }
            drop(active);
            changed.await;
        }
    }

    pub(super) async fn suspend(&self) {
        *self.active.write().await = false;
    }

    pub(super) async fn resume(&self) {
        *self.active.write().await = true;
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::BackgroundActivity;
    use std::sync::Arc;

    #[tokio::test]
    async fn suspension_drains_current_disk_work_and_holds_new_work_until_resume() {
        let activity = Arc::new(BackgroundActivity::new());
        let writing = activity.enter().await;
        let suspending = tokio::spawn({
            let activity = Arc::clone(&activity);
            async move { activity.suspend().await }
        });
        tokio::task::yield_now().await;
        assert!(!suspending.is_finished());
        drop(writing);
        suspending.await.unwrap();
        let next_write = tokio::spawn({
            let activity = Arc::clone(&activity);
            async move {
                let _permit = activity.enter().await;
            }
        });
        tokio::task::yield_now().await;
        assert!(!next_write.is_finished());
        activity.resume().await;
        next_write.await.unwrap();
    }
}
