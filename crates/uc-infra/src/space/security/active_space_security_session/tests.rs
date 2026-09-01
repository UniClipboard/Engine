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
struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
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
