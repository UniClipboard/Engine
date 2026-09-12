use std::fs::OpenOptions;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{oneshot, Notify};
use tokio::time::timeout;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{ready_material, MemorySecureStorage, ProfileContentKeyVault};

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
    let root = tempfile::tempdir().unwrap();
    let (release, blocked) = oneshot::channel();
    let storage = Arc::new(HeldStorage {
        memory: MemorySecureStorage::default(),
        entered: Notify::new(),
        release: Mutex::new(Some(blocked)),
    });
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.path().into(),
        storage.clone(),
        [7; 16],
    ));
    let _reuse = vault.begin_read_reuse().unwrap();
    let installing = tokio::spawn({
        let vault = vault.clone();
        async move {
            vault
                .install_verified_space_material(&ready_material("space", "group", "key", 1, 7))
                .await
        }
    });
    storage.entered.notified().await;
    let competing = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("profile-content-key-vault.lease"))
        .unwrap();
    assert!(competing.try_lock().is_err());
    let mut stopping = Box::pin(vault.suspend());
    let premature = timeout(Duration::from_millis(20), stopping.as_mut()).await;
    release.send(()).unwrap();
    assert!(premature.is_err());
    installing.await.unwrap().unwrap();
    stopping.await;
    competing.try_lock().unwrap();
}
