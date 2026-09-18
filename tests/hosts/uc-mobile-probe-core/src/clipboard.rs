use std::sync::{Arc, Condvar, Mutex};

use async_trait::async_trait;
use tokio::sync::{broadcast, Notify};
use uc_engine::{
    HostCapabilityError, HostClipboard, HostClipboardChange, HostClipboardChangeStream,
    HostClipboardRepresentation, HostClipboardSnapshot,
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
    write_started: Notify,
    changes: broadcast::Sender<()>,
}

struct ClipboardState {
    snapshot: HostClipboardSnapshot,
    block_next_read: bool,
    read_blocked: bool,
    block_next_write: bool,
    write_blocked: bool,
}

impl Default for ProbeClipboard {
    fn default() -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            inner: Arc::new(ClipboardControl {
                state: Mutex::new(ClipboardState {
                    snapshot: HostClipboardSnapshot {
                        observed_at_ms: 0,
                        representations: Vec::new(),
                    },
                    block_next_read: false,
                    read_blocked: false,
                    block_next_write: false,
                    write_blocked: false,
                }),
                released: Condvar::new(),
                read_started: Notify::new(),
                write_started: Notify::new(),
                changes,
            }),
        }
    }
}

impl ProbeClipboard {
    pub(super) fn prepare_blocked_text_read(&self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        state.snapshot = text_snapshot("mobile lifecycle probe", 1);
        state.block_next_read = true;
        state.read_blocked = false;
    }

    pub(super) fn publish_text_change(&self, text: &str) -> bool {
        let mut state = lock_unpoisoned(&self.inner.state);
        state.snapshot = text_snapshot(text, 2);
        drop(state);
        self.inner.changes.send(()).is_ok()
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

    pub(super) fn prepare_blocked_write(&self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        state.block_next_write = true;
        state.write_blocked = false;
    }

    pub(super) async fn wait_until_write_starts(&self) {
        loop {
            let started = self.inner.write_started.notified();
            if lock_unpoisoned(&self.inner.state).write_blocked {
                return;
            }
            started.await;
        }
    }

    pub(super) fn release_write(&self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        state.write_blocked = false;
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
        let mut state = lock_unpoisoned(&self.inner.state);
        if state.block_next_write {
            state.block_next_write = false;
            state.write_blocked = true;
            self.inner.write_started.notify_waiters();
            while state.write_blocked {
                state = self
                    .inner
                    .released
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        Ok(())
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(Some(Box::new(ProbeClipboardChanges {
            receiver: self.inner.changes.subscribe(),
        })))
    }
}

struct ProbeClipboardChanges {
    receiver: broadcast::Receiver<()>,
}

#[async_trait]
impl HostClipboardChangeStream for ProbeClipboardChanges {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        loop {
            match self.receiver.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    return Ok(HostClipboardChange::Changed);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Ok(HostClipboardChange::Closed);
                }
            }
        }
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

fn text_snapshot(text: &str, observed_at_ms: i64) -> HostClipboardSnapshot {
    HostClipboardSnapshot {
        observed_at_ms,
        representations: vec![HostClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: text.as_bytes().to_vec(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{HostClipboard, HostClipboardChange, HostClipboardSnapshot, ProbeClipboard};

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

    #[tokio::test]
    async fn controlled_write_stays_blocked_until_released() {
        let clipboard = ProbeClipboard::default();
        clipboard.prepare_blocked_write();
        let writing = tokio::task::spawn_blocking({
            let clipboard = clipboard.clone();
            move || {
                clipboard.write(HostClipboardSnapshot {
                    observed_at_ms: 1,
                    representations: Vec::new(),
                })
            }
        });
        tokio::time::timeout(Duration::from_secs(1), clipboard.wait_until_write_starts())
            .await
            .unwrap();
        assert!(!writing.is_finished());
        clipboard.release_write();
        writing.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn text_change_is_published_to_each_engine_session() {
        let clipboard = ProbeClipboard::default();
        let mut first_host = clipboard.clone();
        let mut first = first_host.take_change_stream().unwrap().unwrap();
        assert!(clipboard.publish_text_change("first"));
        assert_eq!(first.next().await.unwrap(), HostClipboardChange::Changed);
        first.shutdown().await.unwrap();

        let mut second_host = clipboard.clone();
        let mut second = second_host.take_change_stream().unwrap().unwrap();
        assert!(clipboard.publish_text_change("second"));
        assert_eq!(second.next().await.unwrap(), HostClipboardChange::Changed);
        let snapshot = clipboard.read().unwrap();
        assert_eq!(snapshot.representations.len(), 1);
    }
}
