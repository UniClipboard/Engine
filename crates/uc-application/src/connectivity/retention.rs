//! 有界空闲保留与实际交换活动；入站 accept 等待不延长空闲期限。
use super::connection::ConnectionRecord;
use super::ConnectivityError;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub struct IdleRetentionPolicy {
    pub timeout: Duration,
    pub capacity: usize,
}

#[derive(Default)]
struct ActivityState {
    users: usize,
    idle_since: Option<Instant>,
    retired: bool,
}
#[derive(Default)]
pub(super) struct ConnectionActivity {
    state: Mutex<ActivityState>,
    pub changed: Notify,
}
pub(super) struct ActivityGuard(Arc<ConnectionActivity>);
impl ConnectionActivity {
    pub fn acquire(self: &Arc<Self>) -> Result<Arc<ActivityGuard>, ConnectivityError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.retired {
            return Err(ConnectivityError::Stopped);
        }
        state.users += 1;
        state.idle_since = None;
        self.changed.notify_one();
        Ok(Arc::new(ActivityGuard(self.clone())))
    }
    pub fn idle_since(&self) -> Option<Instant> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.users == 0 {
            Some(*state.idle_since.get_or_insert_with(Instant::now))
        } else {
            None
        }
    }
    pub fn retire_if_idle(&self, record: &ConnectionRecord, before: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.users != 0 || state.idle_since.is_none_or(|since| since > before) {
            return false;
        }
        state.retired = true;
        record.connection.close();
        true
    }
}
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.users -= 1;
        if state.users == 0 {
            state.idle_since = Some(Instant::now());
        }
        self.0.changed.notify_one();
    }
}
