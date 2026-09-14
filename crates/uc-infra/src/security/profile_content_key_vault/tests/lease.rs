use std::fs::OpenOptions;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::timeout;

use super::super::persistence::LeaseProbe;
use super::{MemorySecureStorage, ProfileContentKeyVault, ProfileContentKeyVaultError};

#[tokio::test]
async fn acquiring_the_vault_lease_does_not_block_runtime_notifications() {
    let directory = tempfile::tempdir().unwrap();
    let vault = ProfileContentKeyVault::new(
        directory.path().into(),
        Arc::new(MemorySecureStorage::default()),
        [3; 16],
    );
    vault.suspend().await;
    let (release, blocked) = mpsc::channel();
    *vault.persistence.before_lease.lock().unwrap() = Some(LeaseProbe {
        entered: Arc::new(Notify::new()),
        release: blocked,
    });
    let mut resuming = Box::pin(vault.resume());
    let pending = timeout(Duration::from_millis(20), resuming.as_mut()).await;
    let _ = release.send(());
    assert!(
        pending.is_err(),
        "lease access must not block lifecycle notifications"
    );
    resuming.await.unwrap();
    let competing = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.path().join("profile-content-key-vault.lease"))
        .unwrap();
    assert!(competing.try_lock().is_err());
    vault.suspend().await;
    competing.try_lock().unwrap();
}

#[tokio::test]
async fn abandoned_resume_keeps_file_access_owned_and_never_reopens_a_closed_vault() {
    for close in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(ProfileContentKeyVault::new(
            directory.path().into(),
            Arc::new(MemorySecureStorage::default()),
            [3; 16],
        ));
        vault.suspend().await;
        let entered = Arc::new(Notify::new());
        let (release, blocked) = mpsc::channel();
        *vault.persistence.before_lease.lock().unwrap() = Some(LeaseProbe {
            entered: entered.clone(),
            release: blocked,
        });
        let resuming = tokio::spawn({
            let vault = vault.clone();
            async move { vault.resume().await }
        });
        entered.notified().await;
        resuming.abort();
        assert!(resuming.await.unwrap_err().is_cancelled());
        if close {
            vault.close();
        }
        let mut stopping = Box::pin(vault.suspend());
        let pending = timeout(Duration::from_millis(20), stopping.as_mut()).await;
        let _ = release.send(());
        assert!(pending.is_err());
        stopping.await;
        let competing = OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join("profile-content-key-vault.lease"))
            .unwrap();
        competing.try_lock().unwrap();
        drop(competing);
        if close {
            assert!(matches!(
                vault.resume().await,
                Err(ProfileContentKeyVaultError::Closed)
            ));
        } else {
            vault.resume().await.unwrap();
            vault.suspend().await;
        }
    }
}
