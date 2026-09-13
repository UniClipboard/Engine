use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{Notify, RwLock, RwLockReadGuard};

/// 一次完整磁盘动作持有读许可；暂停等待已有动作结束并阻止后续动作。
pub(super) struct BackgroundActivity {
    active: RwLock<bool>,
    generation: AtomicU64,
    changed: Notify,
}

impl BackgroundActivity {
    pub(super) fn new() -> Self {
        Self {
            active: RwLock::new(true),
            generation: AtomicU64::new(0),
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

    pub(super) fn timer_ticket(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub(super) async fn enter_timer(&self, ticket: u64) -> Option<RwLockReadGuard<'_, bool>> {
        let active = self.active.read().await;
        if *active && self.generation.load(Ordering::SeqCst) == ticket {
            Some(active)
        } else {
            None
        }
    }

    pub(super) async fn suspend(&self) {
        let mut active = self.active.write().await;
        if *active {
            self.generation.fetch_add(1, Ordering::SeqCst);
            *active = false;
        }
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
    async fn timer_from_before_suspension_stays_stale_after_resume() {
        let activity = BackgroundActivity::new();
        let ticket = activity.timer_ticket();

        activity.suspend().await;
        activity.resume().await;

        assert!(activity.enter_timer(ticket).await.is_none());
        let current_ticket = activity.timer_ticket();
        assert!(activity.enter_timer(current_ticket).await.is_some());
    }

    #[tokio::test]
    async fn timer_firing_while_suspended_is_discarded() {
        let activity = BackgroundActivity::new();
        activity.suspend().await;

        let ticket = activity.timer_ticket();

        assert!(activity.enter_timer(ticket).await.is_none());
    }

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
