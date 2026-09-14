use std::sync::{Arc, Condvar, Mutex};

use tokio::sync::Notify;
use uc_engine::{HostCapabilityError, HostSecureStorage};

use super::lock_unpoisoned;

#[derive(Default)]
struct SecureStorageControlState {
    block_next_get: bool,
    get_blocked: bool,
}

struct SecureStorageControl {
    state: Mutex<SecureStorageControlState>,
    released: Condvar,
    get_started: Notify,
}

#[derive(Clone)]
pub(super) struct ControlledSecureStorage {
    inner: Arc<dyn HostSecureStorage>,
    control: Arc<SecureStorageControl>,
}

impl ControlledSecureStorage {
    pub(super) fn new(inner: Box<dyn HostSecureStorage>) -> Self {
        Self {
            inner: Arc::from(inner),
            control: Arc::new(SecureStorageControl {
                state: Mutex::new(SecureStorageControlState::default()),
                released: Condvar::new(),
                get_started: Notify::new(),
            }),
        }
    }

    pub(super) fn prepare_blocked_get(&self) {
        let mut state = lock_unpoisoned(&self.control.state);
        state.block_next_get = true;
        state.get_blocked = false;
    }

    pub(super) async fn wait_until_get_starts(&self) {
        loop {
            let started = self.control.get_started.notified();
            if lock_unpoisoned(&self.control.state).get_blocked {
                return;
            }
            started.await;
        }
    }

    pub(super) fn release_get(&self) {
        let mut state = lock_unpoisoned(&self.control.state);
        state.get_blocked = false;
        self.control.released.notify_all();
    }

    fn wait_if_get_blocked(&self) {
        let mut state = lock_unpoisoned(&self.control.state);
        if state.block_next_get {
            state.block_next_get = false;
            state.get_blocked = true;
            self.control.get_started.notify_waiters();
            while state.get_blocked {
                state = self
                    .control
                    .released
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

impl HostSecureStorage for ControlledSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        self.wait_if_get_blocked();
        self.inner.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.inner.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.inner.delete(key)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use uc_engine::{HostCapabilityError, HostSecureStorage};

    use super::ControlledSecureStorage;

    #[derive(Default)]
    struct MemorySecureStorage {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl HostSecureStorage for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn controlled_get_stays_blocked_until_released() {
        let storage = ControlledSecureStorage::new(Box::<MemorySecureStorage>::default());
        storage.set("key", b"value").unwrap();
        storage.prepare_blocked_get();
        let reading = tokio::task::spawn_blocking({
            let storage = storage.clone();
            move || storage.get("key")
        });
        tokio::time::timeout(Duration::from_secs(1), storage.wait_until_get_starts())
            .await
            .unwrap();
        assert!(!reading.is_finished());
        storage.release_get();
        assert_eq!(reading.await.unwrap().unwrap(), Some(b"value".to_vec()));
    }
}
