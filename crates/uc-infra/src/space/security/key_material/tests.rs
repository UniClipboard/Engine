use std::error::Error;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use tokio::task::JoinError;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{EncryptionError, Kek, KeyMaterialStore, KeyScope};
use crate::fs::key_slot_store::{JsonKeySlotStore, KeySlotStore};
use crate::security::crypto_model::{
    EncryptedBlob, KeySlot, WrappedMasterKey, MAX_KDF_ITERS, MAX_KDF_MEM_KIB, MAX_KDF_PARALLELISM,
};

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

#[tokio::test]
async fn keyslot_rejects_unbounded_kdf_parameters_before_derivation() {
    for (mem_kib, iters, parallelism) in [
        (MAX_KDF_MEM_KIB + 1, 3, 4),
        (128 * 1024, MAX_KDF_ITERS + 1, 4),
        (128 * 1024, 3, MAX_KDF_PARALLELISM + 1),
        (128 * 1024, 5, 4),
    ] {
        let root = tempfile::tempdir().unwrap();
        let scope = KeyScope {
            profile_id: "test".into(),
        };
        let mut slot = KeySlot::draft_v1(scope.clone()).unwrap();
        slot.kdf.params.mem_kib = mem_kib;
        slot.kdf.params.iters = iters;
        slot.kdf.params.parallelism = parallelism;
        let slot = slot.finalize(WrappedMasterKey {
            blob: EncryptedBlob {
                version: "V1".to_owned(),
                aead: "XChaCha20Poly1305".to_owned(),
                nonce: vec![0; 24],
                ciphertext: vec![0; 48],
                aad_fingerprint: None,
            },
        });
        let store = Arc::new(JsonKeySlotStore::new(root.path().into()));
        store.store(&(&slot).try_into().unwrap()).await.unwrap();
        let material = KeyMaterialStore::new(
            Arc::new(TestStorage {
                threads: Mutex::new(Vec::new()),
                panic: false,
            }),
            store,
        );
        assert!(matches!(
            material.load_keyslot(&scope).await,
            Err(EncryptionError::CorruptedKeySlot)
        ));
    }
}
