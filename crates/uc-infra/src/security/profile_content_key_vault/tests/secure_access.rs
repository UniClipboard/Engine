use std::fs::OpenOptions;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{oneshot, Notify};
use tokio::task::JoinError;
use tokio::time::timeout;
use uc_core::membership::{ContentKeyId, GroupEpoch};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{
    ready_material, MemorySecureStorage, ProfileContentKeyVault, ProfileContentKeyVaultError,
};

#[tokio::test]
async fn owned_access_panic_keeps_its_source_and_safe_summary() {
    let root = tempfile::tempdir().unwrap();
    let vault = ProfileContentKeyVault::new(
        root.path().into(),
        Arc::new(MemorySecureStorage::default()),
        [7; 16],
    );
    let result: Result<(), ProfileContentKeyVaultError> = vault
        .run_owned(|_owner| async {
            panic!("PRIVATE vault action panic");
        })
        .await;
    let failure = result.unwrap_err();
    assert!(failure.source().is_some());
    assert!(!format!("{failure:?}").contains("PRIVATE"));
    let ProfileContentKeyVaultError::Storage { source } = failure else {
        panic!("expected storage error");
    };
    assert!(source.downcast_ref::<JoinError>().unwrap().is_panic());
}

struct HeldStorage {
    memory: MemorySecureStorage,
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl SecureStoragePort for HeldStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        let release = self.release.lock().unwrap().take();
        if let Some(release) = release {
            self.entered.notify_one();
            release.blocking_recv().unwrap();
        }
        self.memory.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.memory.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.memory.delete(key)
    }
}

#[tokio::test]
async fn suspend_keeps_waiting_for_secure_storage_before_releasing_the_vault_lease() {
    check_suspend_during_secure_storage(false, false).await;
}

#[tokio::test]
async fn cancelling_install_waiter_does_not_release_an_active_secure_storage_operation() {
    check_suspend_during_secure_storage(true, false).await;
}

#[tokio::test]
async fn cancelling_cold_read_waiter_does_not_release_an_active_secure_storage_operation() {
    check_suspend_during_secure_storage(true, true).await;
}

async fn check_suspend_during_secure_storage(cancel_waiter: bool, cold_read: bool) {
    let root = tempfile::tempdir().unwrap();
    let (release, blocked) = oneshot::channel();
    let storage = Arc::new(HeldStorage {
        memory: MemorySecureStorage::default(),
        entered: Notify::new(),
        release: Mutex::new(None),
    });
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.path().into(),
        storage.clone(),
        [7; 16],
    ));
    if cold_read {
        vault
            .install_verified_space_material(&ready_material("space", "group", "key", 1, 7))
            .await
            .unwrap();
    }
    *storage.release.lock().unwrap() = Some(blocked);
    let _reuse = vault.begin_read_reuse().unwrap();
    let installing = tokio::spawn({
        let vault = vault.clone();
        async move {
            if cold_read {
                vault
                    .resolve(
                        &ContentKeyId::from_string("key").unwrap(),
                        GroupEpoch::new(1),
                    )
                    .await
                    .map(|_| ())
            } else {
                vault
                    .install_verified_space_material(&ready_material("space", "group", "key", 1, 7))
                    .await
                    .map(|_| ())
            }
        }
    });
    storage.entered.notified().await;
    if cancel_waiter {
        installing.abort();
    }
    let competing = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("profile-content-key-vault.lease"))
        .unwrap();
    let locked_during_access = competing.try_lock().is_err();
    let mut stopping = Box::pin(vault.suspend());
    let premature = timeout(Duration::from_millis(20), stopping.as_mut()).await;
    release.send(()).unwrap();
    assert!(locked_during_access);
    assert!(premature.is_err());
    if cancel_waiter {
        assert!(installing.await.unwrap_err().is_cancelled());
    } else {
        installing.await.unwrap().unwrap();
    }
    stopping.await;
    competing.try_lock().unwrap();
    drop(competing);
    vault.resume().await.unwrap();
    assert_eq!(
        vault
            .resolve(
                &ContentKeyId::from_string("key").unwrap(),
                GroupEpoch::new(1)
            )
            .await
            .unwrap()
            .key()
            .as_bytes(),
        &[7; 32]
    );
}
use std::error::Error as _;
