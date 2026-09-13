use std::sync::{Arc, Condvar, Mutex};

use tokio::sync::Notify;
use uc_engine::{
    HostCapabilityError, HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot,
};

use super::lock_unpoisoned;

#[derive(Clone)]
pub(super) struct ProbeClipboard {
    inner: Arc<ClipboardControl>,
}

struct ClipboardControl {
    state: Mutex<ClipboardState>,
    released: Condvar,
    read_started: Notify,
}

struct ClipboardState {
    snapshot: HostClipboardSnapshot,
    block_next_read: bool,
    read_blocked: bool,
}

impl Default for ProbeClipboard {
    fn default() -> Self {
        Self {
            inner: Arc::new(ClipboardControl {
                state: Mutex::new(ClipboardState {
                    snapshot: HostClipboardSnapshot {
                        observed_at_ms: 0,
                        representations: Vec::new(),
                    },
                    block_next_read: false,
                    read_blocked: false,
                }),
                released: Condvar::new(),
                read_started: Notify::new(),
            }),
        }
    }
}

impl ProbeClipboard {
    pub(super) fn prepare_blocked_text_read(&self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        state.snapshot = HostClipboardSnapshot {
            observed_at_ms: 1,
            representations: vec![HostClipboardRepresentation::Inline {
                format: "text/plain".to_owned(),
                mime_type: Some("text/plain".to_owned()),
                bytes: b"mobile lifecycle probe".to_vec(),
            }],
        };
        state.block_next_read = true;
        state.read_blocked = false;
    }

    pub(super) async fn wait_until_read_starts(&self) {
        loop {
            let started = self.inner.read_started.notified();
            if lock_unpoisoned(&self.inner.state).read_blocked {
                return;
            }
            started.await;
        }
    }

    pub(super) fn release_read(&self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        state.read_blocked = false;
        self.inner.released.notify_all();
    }
}

impl HostClipboard for ProbeClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        let mut state = lock_unpoisoned(&self.inner.state);
        if state.block_next_read {
            state.block_next_read = false;
            state.read_blocked = true;
            self.inner.read_started.notify_waiters();
            while state.read_blocked {
                state = self
                    .inner
                    .released
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        Ok(state.snapshot.clone())
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{HostClipboard, ProbeClipboard};

    #[tokio::test]
    async fn controlled_read_stays_blocked_until_released() {
        let clipboard = ProbeClipboard::default();
        clipboard.prepare_blocked_text_read();
        let reading = tokio::task::spawn_blocking({
            let clipboard = clipboard.clone();
            move || clipboard.read()
        });
        tokio::time::timeout(Duration::from_secs(1), clipboard.wait_until_read_starts())
            .await
            .unwrap();
        assert!(!reading.is_finished());
        clipboard.release_read();
        let snapshot = reading.await.unwrap().unwrap();
        assert_eq!(snapshot.representations.len(), 1);
    }
}
