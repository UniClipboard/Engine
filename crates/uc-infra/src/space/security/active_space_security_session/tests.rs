use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tempfile::tempdir;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::ActiveSpaceSecuritySession;
use crate::security::{MasterKey, ProfileContentKeyVault};
use crate::space::security::InMemorySession;

#[derive(Default)]
struct MemorySecureStorage(
    Mutex<BTreeMap<String, Vec<u8>>>,
    std::sync::atomic::AtomicUsize,
    Mutex<Option<Box<dyn FnOnce() + Send>>>,
);

impl SecureStoragePort for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let hook = self.2.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[derive(Serialize)]
struct CatalogFixture {
    version: u8,
    entries: Vec<CatalogEntryFixture>,
}

#[derive(Serialize)]
struct CatalogEntryFixture {
    content_key_id: String,
    epoch: u64,
    key: Vec<u8>,
}

fn ready_material(space_id: &str, group_id: &str, key_id: &str) -> SpaceKeyMaterial {
    let epoch = GroupEpoch::new(7);
    let state = SpaceKeyState::ready_for_admission(
        SpaceId::from(space_id),
        epoch,
        ContentKeyId::from_string(key_id).unwrap(),
        ProtectionGroupId::from_string(group_id).unwrap(),
    )
    .unwrap();
    let catalog = CatalogFixture {
        version: 2,
        entries: vec![
            CatalogEntryFixture {
                content_key_id: "legacy-v1".to_owned(),
                epoch: 0,
                key: vec![0x20; 32],
            },
            CatalogEntryFixture {
                content_key_id: key_id.to_owned(),
                epoch: epoch.value(),
                key: vec![0x31; 32],
            },
        ],
    };
    SpaceKeyMaterial::new(
        state,
        b"verified-group-state".to_vec(),
        serde_json::to_vec(&catalog).unwrap(),
        1,
    )
}

fn advanced_material(
    space_id: &str,
    group_id: &str,
    previous_key_id: &str,
    current_key_id: &str,
) -> SpaceKeyMaterial {
    let epoch = GroupEpoch::new(8);
    let state = SpaceKeyState::ready_for_admission(
        SpaceId::from(space_id),
        epoch,
        ContentKeyId::from_string(current_key_id).unwrap(),
        ProtectionGroupId::from_string(group_id).unwrap(),
    )
    .unwrap();
    let catalog = CatalogFixture {
        version: 2,
        entries: vec![
            CatalogEntryFixture {
                content_key_id: "legacy-v1".to_owned(),
                epoch: 0,
                key: vec![0x20; 32],
            },
            CatalogEntryFixture {
                content_key_id: previous_key_id.to_owned(),
                epoch: 7,
                key: vec![0x31; 32],
            },
            CatalogEntryFixture {
                content_key_id: current_key_id.to_owned(),
                epoch: epoch.value(),
                key: vec![0x32; 32],
            },
        ],
    };
    SpaceKeyMaterial::new(
        state,
        b"advanced-group-state".to_vec(),
        serde_json::to_vec(&catalog).unwrap(),
        2,
    )
}

fn active_fixture() -> (
    tempfile::TempDir,
    Arc<InMemorySession>,
    Arc<ProfileContentKeyVault>,
    ActiveSpaceSecuritySession,
) {
    let directory = tempdir().unwrap();
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().to_path_buf(),
        Arc::new(MemorySecureStorage::default()),
        [0x11; 16],
    ));
    let session = Arc::new(InMemorySession::new());
    let active = ActiveSpaceSecuritySession::new(Arc::clone(&session), Arc::clone(&vault));
    (directory, session, vault, active)
}

#[tokio::test]
async fn activation_installs_catalog_before_switching_the_active_session() {
    let (_directory, session, vault, active) = active_fixture();
    let material = ready_material("space-b", "group-b", "key-b");

    active
        .activate(
            &SpaceId::from("space-b"),
            MasterKey::from_bytes(&[0x42; 32]).unwrap(),
            Some(&material),
        )
        .await
        .unwrap();

    assert_eq!(session.current_space_id().unwrap().as_ref(), "space-b");
    let resolved = vault
        .resolve(
            &ContentKeyId::from_string("key-b").unwrap(),
            GroupEpoch::new(7),
        )
        .await
        .unwrap();
    assert_eq!(resolved.protection_group_id().as_str(), "group-b");
}

#[tokio::test]
async fn vault_failure_preserves_the_previous_active_session_and_source() {
    let (_directory, session, _vault, active) = active_fixture();
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x41; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "shared-key")),
        )
        .await
        .unwrap();

    let error = active
        .activate(
            &SpaceId::from("space-b"),
            MasterKey::from_bytes(&[0x42; 32]).unwrap(),
            Some(&ready_material("space-b", "group-b", "shared-key")),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        super::ActiveSpaceSecuritySessionError::Vault { .. }
    ));
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(session.current_space_id().unwrap().as_ref(), "space-a");
}

#[tokio::test]
async fn mismatched_material_is_rejected_before_vault_or_session_changes() {
    let (_directory, session, vault, active) = active_fixture();

    let error = active
        .activate(
            &SpaceId::from("space-b"),
            MasterKey::from_bytes(&[0x42; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "key-a")),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        super::ActiveSpaceSecuritySessionError::InvalidMaterial { .. }
    ));
    assert!(std::error::Error::source(&error).is_some());
    assert!(session.current_space_id().is_err());
    assert!(vault
        .resolve(
            &ContentKeyId::from_string("key-a").unwrap(),
            GroupEpoch::new(7),
        )
        .await
        .is_err());
}

#[tokio::test]
async fn legacy_activation_switches_session_without_creating_a_catalog() {
    let (_directory, session, vault, active) = active_fixture();

    active
        .activate(
            &SpaceId::from("legacy-space"),
            MasterKey::from_bytes(&[0x51; 32]).unwrap(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(session.current_space_id().unwrap().as_ref(), "legacy-space");
    assert!(vault
        .resolve(
            &ContentKeyId::from_string("unknown-key").unwrap(),
            GroupEpoch::new(1),
        )
        .await
        .is_err());
}

#[tokio::test]
async fn current_material_update_installs_catalog_before_advancing_the_session() {
    let (_directory, session, vault, active) = active_fixture();
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x41; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "key-a")),
        )
        .await
        .unwrap();

    let advanced = advanced_material("space-a", "group-a", "key-a", "key-b");
    active.install_current_material(&advanced).await.unwrap();

    let current = session.current_content_protection_key().unwrap();
    assert_eq!(current.content_key_id().as_str(), "key-b");
    assert_eq!(current.epoch(), GroupEpoch::new(8));
    assert!(vault
        .resolve(
            &ContentKeyId::from_string("key-b").unwrap(),
            GroupEpoch::new(8),
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn current_material_vault_failure_preserves_the_previous_session_and_source() {
    let (_directory, session, vault, active) = active_fixture();
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x41; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "key-a")),
        )
        .await
        .unwrap();
    vault
        .install_verified_space_material(&ready_material("space-b", "group-b", "conflicting-key"))
        .await
        .unwrap();

    let error = active
        .install_current_material(&advanced_material(
            "space-a",
            "group-a",
            "key-a",
            "conflicting-key",
        ))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        super::ActiveSpaceSecuritySessionError::Vault { .. }
    ));
    assert!(std::error::Error::source(&error).is_some());
    let current = session.current_content_protection_key().unwrap();
    assert_eq!(current.content_key_id().as_str(), "key-a");
    assert_eq!(current.epoch(), GroupEpoch::new(7));
}

#[tokio::test]
async fn active_content_reads_reuse_profile_keys_across_payloads_and_search() {
    use crate::security::ContentProtection;
    use std::sync::atomic::Ordering;
    use uc_core::crypto::domain::{Aad, Plaintext};

    let directory = tempdir().unwrap();
    let storage = Arc::new(MemorySecureStorage::default());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().into(),
        storage.clone(),
        [19; 16],
    ));
    let session = Arc::new(InMemorySession::new());
    let active = ActiveSpaceSecuritySession::new(session.clone(), vault.clone());
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "key-a")),
        )
        .await
        .unwrap();
    let protection = ContentProtection::for_content(session.clone(), vault.clone());
    let aad = Aad::new(b"fixture".to_vec());
    let ciphertext = protection
        .seal_for_active(&Plaintext::new(b"protected fixture".to_vec()), &aad)
        .await
        .unwrap();
    let before = storage.1.load(Ordering::SeqCst);
    for _ in 0..20 {
        assert_eq!(
            protection.open(&ciphertext, &aad).await.unwrap().as_bytes(),
            b"protected fixture"
        );
        vault.search_catalog().await.unwrap();
    }
    assert_eq!(
        storage.1.load(Ordering::SeqCst) - before,
        1,
        "one cold load must serve all payload and search reads"
    );
}

#[tokio::test]
async fn clear_releases_reuse_but_historical_maintenance_reads_remain_available() {
    let (directory, session, vault, active) = active_fixture();
    let material = ready_material("space-a", "group-a", "key-a");
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
            Some(&material),
        )
        .await
        .unwrap();
    let id = ContentKeyId::from_string("key-a").unwrap();
    vault.resolve(&id, GroupEpoch::new(7)).await.unwrap();
    let contender = ProfileContentKeyVault::new(
        directory.path().into(),
        Arc::new(MemorySecureStorage::default()),
        [17; 16],
    );
    let error = contender
        .install_verified_space_material(&material)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::security::ProfileContentKeyVaultError::Storage { .. }
    ));
    assert!(std::error::Error::source(&error).is_some());
    session.clear();
    assert!(!session.is_ready());
    // 维护读取继续可用，但不恢复活动 session。
    vault.resolve(&id, GroupEpoch::new(7)).await.unwrap();
    assert!(!session.is_ready());
    // 未持有运行期租约，另一个实例可以进入并报告其真正的缺钥错误。
    assert!(matches!(
        contender.resolve(&id, GroupEpoch::new(7)).await,
        Err(crate::security::ProfileContentKeyVaultError::Corrupt { .. })
    ));
}

#[tokio::test]
async fn closed_profile_cannot_reactivate_or_classify_read_as_corrupt() {
    use crate::security::{ContentProtection, ContentProtectionError};
    use uc_core::crypto::domain::{Aad, Plaintext};
    let (_directory, session, vault, active) = active_fixture();
    let material = ready_material("space-a", "group-a", "key-a");
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
            Some(&material),
        )
        .await
        .unwrap();
    let protection = ContentProtection::for_content(session.clone(), vault);
    let aad = Aad::new(b"fixture".to_vec());
    let ciphertext = protection
        .seal_for_active(&Plaintext::new(b"fixture".to_vec()), &aad)
        .await
        .unwrap();
    active.close();
    assert!(matches!(
        protection.open(&ciphertext, &aad).await,
        Err(ContentProtectionError::NotActive { .. })
    ));
    assert!(active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
            Some(&material)
        )
        .await
        .is_err());
    assert!(!session.is_ready());
}

#[tokio::test]
async fn failed_or_cancelled_activation_cannot_resurrect_a_cleared_session() {
    let (_directory, session, vault, active) = active_fixture();
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "key-a")),
        )
        .await
        .unwrap();
    let transaction = session
        .begin_transaction(Some((
            SpaceId::from("space-b"),
            MasterKey::from_bytes(&[33; 32]).unwrap(),
        )))
        .unwrap();
    drop(transaction);
    assert_eq!(
        session.current_space_id().unwrap(),
        SpaceId::from("space-a")
    );
    let transaction = session
        .begin_transaction(Some((
            SpaceId::from("space-b"),
            MasterKey::from_bytes(&[33; 32]).unwrap(),
        )))
        .unwrap();
    session.clear();
    assert!(transaction.commit(&vault).is_err());
    assert!(!session.is_ready());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_load_finishing_after_clear_does_not_repopulate_reuse() {
    use std::sync::atomic::Ordering;
    let directory = tempdir().unwrap();
    let storage = Arc::new(MemorySecureStorage::default());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().into(),
        storage.clone(),
        [19; 16],
    ));
    let session = Arc::new(InMemorySession::new());
    let active = ActiveSpaceSecuritySession::new(session.clone(), vault.clone());
    active
        .activate(
            &SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
            Some(&ready_material("space-a", "group-a", "key-a")),
        )
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(std::sync::Barrier::new(2));
    let notify = entered.clone();
    let gate = release.clone();
    *storage.2.lock().unwrap() = Some(Box::new(move || {
        notify.notify_one();
        gate.wait();
    }));
    let reader = vault.clone();
    let read = tokio::spawn(async move { reader.search_catalog().await });
    entered.notified().await;
    session.clear();
    release.wait();
    read.await.unwrap().unwrap();
    let before = storage.1.load(Ordering::SeqCst);
    vault.search_catalog().await.unwrap();
    vault.search_catalog().await.unwrap();
    assert_eq!(storage.1.load(Ordering::SeqCst) - before, 2);
    assert!(!session.is_ready());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starting_reuse_during_a_transient_read_requires_a_new_owned_load() {
    use std::sync::atomic::Ordering;
    let directory = tempdir().unwrap();
    let storage = Arc::new(MemorySecureStorage::default());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().into(),
        storage.clone(),
        [19; 16],
    ));
    let material = ready_material("space-a", "group-a", "key-a");
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(std::sync::Barrier::new(2));
    let notify = entered.clone();
    let gate = release.clone();
    *storage.2.lock().unwrap() = Some(Box::new(move || {
        notify.notify_one();
        gate.wait();
    }));
    let reader = vault.clone();
    let read = tokio::spawn(async move { reader.search_catalog().await });
    entered.notified().await;
    // 激活发生在旧临时读取已取得文件租约之后。
    let session = InMemorySession::new();
    let transaction = session
        .begin_transaction(Some((
            SpaceId::from("space-a"),
            MasterKey::from_bytes(&[32; 32]).unwrap(),
        )))
        .unwrap();
    session.install_space_material(&material).unwrap();
    transaction.commit(&vault).unwrap();
    release.wait();
    read.await.unwrap().unwrap();
    let before = storage.1.load(Ordering::SeqCst);
    vault.search_catalog().await.unwrap();
    vault.search_catalog().await.unwrap();
    assert_eq!(storage.1.load(Ordering::SeqCst) - before, 1);
    let contender = ProfileContentKeyVault::new(directory.path().into(), storage, [19; 16]);
    assert!(matches!(
        contender.search_catalog().await,
        Err(crate::security::ProfileContentKeyVaultError::Storage { .. })
    ));
}
