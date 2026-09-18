use std::error::Error;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use tokio::task::JoinError;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{EncryptionError, Kek, KeyMaterialStore, KeyScope};
use crate::fs::key_slot_store::JsonKeySlotStore;

struct TestStorage {
    threads: Mutex<Vec<ThreadId>>,
    panic: bool,
}

impl TestStorage {
    fn access(&self) {
        self.threads.lock().unwrap().push(thread::current().id());
        assert!(!self.panic, "sensitive host failure");
    }
}

impl SecureStoragePort for TestStorage {
    fn get(&self, _key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        self.access();
        Ok(Some(vec![7; 32]))
    }

    fn set(&self, _key: &str, _value: &[u8]) -> Result<(), SecureStorageError> {
        self.access();
        Ok(())
    }

    fn delete(&self, _key: &str) -> Result<(), SecureStorageError> {
        self.access();
        Ok(())
    }
}

#[tokio::test]
async fn secure_storage_access_runs_outside_the_runtime_thread_and_keeps_failures() {
    let runtime_thread = thread::current().id();
    for panic in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let storage = Arc::new(TestStorage {
            threads: Mutex::new(Vec::new()),
            panic,
        });
        let material = KeyMaterialStore::new(
            storage.clone(),
            Arc::new(JsonKeySlotStore::new(root.path().into())),
        );
        let scope = KeyScope {
            profile_id: "test".into(),
        };
        let kek = Kek::from_bytes(&[7; 32]).unwrap();
        let results = [
            material.load_kek(&scope).await.map(|_| ()),
            material.store_kek(&scope, &kek).await,
            material.delete_kek(&scope).await,
        ];
        for result in results {
            if panic {
                let error = result.unwrap_err();
                assert!(matches!(
                    error,
                    EncryptionError::KeyMaterialAccessFailed { .. }
                ));
                assert!(error
                    .source()
                    .unwrap()
                    .downcast_ref::<JoinError>()
                    .unwrap()
                    .is_panic());
                assert!(!format!("{error:?}").contains("sensitive"));
            } else {
                result.unwrap();
            }
        }
        let threads = storage.threads.lock().unwrap();
        assert_eq!(threads.len(), 3);
        assert!(threads.iter().all(|id| *id != runtime_thread));
    }
}
