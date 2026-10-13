//! 恢复完成后的非阻塞最新值交接。当前轮由 worker 持有，槽只保存下一轮候选。
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;
use tokio::time::Instant;
use uc_core::clipboard::{ActiveClipboardState, ClipboardContentCategorySet};
use uc_core::ids::DeviceId;
use uc_observability_contract::diagnostics::ObservationContext;

#[derive(Clone)]
pub struct RestoreBroadcastRequest {
    pub state: ActiveClipboardState,
    pub categories: ClipboardContentCategorySet,
    pub(crate) context: ObservationContext,
}

pub(crate) struct PendingActivation {
    pub request: RestoreBroadcastRequest,
    pub arrived_at: Instant,
    pub version: u64,
}
struct Slot {
    pending: Option<PendingActivation>,
    // 仅保留排序键，不复制当前轮的业务负载。
    newest_key: Option<(i64, DeviceId)>,
    version: u64,
    senders: usize,
    closed: bool,
}
struct Shared {
    slot: Mutex<Slot>,
    changed: Notify,
}
pub struct RestoreBroadcastTrigger {
    shared: Arc<Shared>,
}
pub struct RestoreBroadcastReceiver {
    shared: Arc<Shared>,
}

pub fn restore_broadcast_channel() -> (RestoreBroadcastTrigger, RestoreBroadcastReceiver) {
    let shared = Arc::new(Shared {
        slot: Mutex::new(Slot {
            pending: None,
            newest_key: None,
            version: 0,
            senders: 1,
            closed: false,
        }),
        changed: Notify::new(),
    });
    (
        RestoreBroadcastTrigger {
            shared: shared.clone(),
        },
        RestoreBroadcastReceiver { shared },
    )
}
impl Clone for RestoreBroadcastTrigger {
    fn clone(&self) -> Self {
        self.shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .senders += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}
impl Drop for RestoreBroadcastTrigger {
    fn drop(&mut self) {
        let mut slot = self
            .shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        slot.senders -= 1;
        drop(slot);
        self.shared.changed.notify_one();
    }
}
impl RestoreBroadcastTrigger {
    /// 状态和类别在同一临界区替换；旧回调与同排序键重复不刷新静默期。
    pub fn offer(&self, state: ActiveClipboardState, categories: ClipboardContentCategorySet) {
        let mut slot = self
            .shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.closed {
            return;
        }
        if let Some((timestamp, device)) = &slot.newest_key {
            if state.activated_at_ms < *timestamp
                || (state.activated_at_ms == *timestamp
                    && state.activated_by.as_str() <= device.as_str())
            {
                return;
            }
        }
        slot.newest_key = Some((state.activated_at_ms, state.activated_by));
        slot.version = slot.version.wrapping_add(1);
        slot.pending = Some(PendingActivation {
            request: RestoreBroadcastRequest {
                state,
                categories,
                context: ObservationContext::capture(),
            },
            arrived_at: Instant::now(),
            version: slot.version,
        });
        drop(slot);
        self.shared.changed.notify_one();
    }
}
impl RestoreBroadcastReceiver {
    /// 原子取走当前版本；网络等待期间不持槽锁，新值属于下一次取走。
    pub(crate) async fn recv(&mut self) -> Option<PendingActivation> {
        loop {
            let changed = self.shared.changed.notified();
            {
                let mut slot = self
                    .shared
                    .slot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if let Some(pending) = slot.pending.take() {
                    return Some(pending);
                }
                if slot.senders == 0 {
                    return None;
                }
            }
            changed.await;
        }
    }
}
impl Drop for RestoreBroadcastReceiver {
    fn drop(&mut self) {
        let mut slot = self
            .shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        slot.closed = true;
        slot.pending.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uc_core::clipboard::ClipboardContentCategory;
    use uc_core::ids::EntryId;

    fn state(version: i64) -> ActiveClipboardState {
        ActiveClipboardState::new(
            format!("hash-{version}"),
            EntryId::new(),
            version,
            DeviceId::new("origin"),
        )
    }
    fn categories(category: ClipboardContentCategory) -> ClipboardContentCategorySet {
        let mut result = ClipboardContentCategorySet::empty();
        result.insert(category);
        result
    }
    #[tokio::test(start_paused = true)]
    async fn consumed_round_does_not_erase_latest_categories_or_restart_elapsed_debounce() {
        let (trigger, mut receiver) = restore_broadcast_channel();
        trigger.offer(state(1), categories(ClipboardContentCategory::Text));
        let current = receiver.recv().await.unwrap();
        for version in 2..=4 {
            trigger.offer(state(version), categories(ClipboardContentCategory::File));
        }
        let arrival = Instant::now();
        tokio::time::advance(Duration::from_millis(500)).await;
        trigger.offer(state(2), categories(ClipboardContentCategory::Text));
        let next = receiver.recv().await.unwrap();
        assert_eq!(current.request.state.activated_at_ms, 1);
        assert_eq!(next.request.state.activated_at_ms, 4);
        assert_eq!(
            next.request.categories,
            categories(ClipboardContentCategory::File)
        );
        assert_eq!(next.arrived_at, arrival);
        assert!(next.arrived_at + Duration::from_millis(300) < Instant::now());
        // 当前值取走后到达的新值必须留给下一次消费，不能被旧轮清槽丢弃。
        trigger.offer(state(5), categories(ClipboardContentCategory::Text));
        assert_eq!(
            receiver.recv().await.unwrap().request.state.activated_at_ms,
            5
        );
        drop(trigger);
        assert!(receiver.recv().await.is_none());
    }
    #[tokio::test(start_paused = true)]
    async fn exact_duplicate_keeps_original_version_and_arrival() {
        let (trigger, mut receiver) = restore_broadcast_channel();
        let value = state(1);
        trigger.offer(value.clone(), categories(ClipboardContentCategory::Text));
        let original = Instant::now();
        tokio::time::advance(Duration::from_millis(299)).await;
        trigger.offer(value, categories(ClipboardContentCategory::Text));
        let pending = receiver.recv().await.unwrap();
        assert_eq!(pending.arrived_at, original);
        assert_eq!(pending.version, 1);
    }
}
