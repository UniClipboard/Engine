use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use tokio::task::JoinError;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{load_existing, load_or_create, ProfileContentKeyVaultError, SecureStorageAccess};

#[derive(Default)]
struct Storage {
    key: Mutex<Option<Vec<u8>>>,
    threads: Mutex<Vec<ThreadId>>,
    panic_on_get: bool,
    panic_on_set: bool,
}

impl SecureStoragePort for Storage {
    fn get(&self, _key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        self.threads.lock().unwrap().push(thread::current().id());
        assert!(!self.panic_on_get, "private host failure");
        Ok(self.key.lock().unwrap().clone())
    }

    fn set(&self, _key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.threads.lock().unwrap().push(thread::current().id());
        assert!(!self.panic_on_set, "private host failure");
        *self.key.lock().unwrap() = Some(value.to_vec());
        Ok(())
    }

    fn delete(&self, _key: &str) -> Result<(), SecureStorageError> {
        panic!("unexpected delete");
    }
}

#[tokio::test]
async fn key_installation_and_readback_run_outside_the_runtime_thread() {
    let storage = Arc::new(Storage::default());
    let access = SecureStorageAccess::new(storage.clone());
    let installed = load_or_create(access.clone()).await.unwrap();
    let read = load_existing(access).await.unwrap();
    assert_eq!(installed, read);
    let threads = storage.threads.lock().unwrap();
    assert_eq!(threads.len(), 4);
    assert!(threads.iter().all(|id| *id != thread::current().id()));
}

#[tokio::test]
async fn host_read_and_write_panics_keep_their_source_without_exposing_payloads() {
    for panic_on_get in [true, false] {
        let storage = Arc::new(Storage {
            panic_on_get,
            panic_on_set: !panic_on_get,
            ..Storage::default()
        });
        let error = load_or_create(SecureStorageAccess::new(storage))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("private"));
        assert!(!format!("{error}").contains("private"));
        let ProfileContentKeyVaultError::SecureStorage { source } = error else {
            panic!("expected secure storage failure");
        };
        assert!(source.downcast_ref::<JoinError>().unwrap().is_panic());
    }
}
