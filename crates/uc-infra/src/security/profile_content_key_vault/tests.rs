use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{ProfileContentKeyVault, ProfileContentKeyVaultError, PROFILE_CONTENT_VAULT_KEY_NAME};

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

fn ready_material(
    space_id: &str,
    protection_group_id: &str,
    content_key_id: &str,
    epoch: u64,
    key_byte: u8,
) -> SpaceKeyMaterial {
    let state = SpaceKeyState::ready_for_admission(
        SpaceId::from_str(space_id),
        GroupEpoch::new(epoch),
        ContentKeyId::from_string(content_key_id).unwrap(),
        ProtectionGroupId::from_string(protection_group_id).unwrap(),
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
                content_key_id: content_key_id.to_owned(),
                epoch,
                key: vec![key_byte; 32],
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

#[tokio::test]
async fn installed_catalog_resolves_exact_key_after_restart_without_plaintext_on_disk() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let material = ready_material("space-a", "group-a", "key-a", 7, 0x31);
    let vault = ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage.clone(),
        [0x11; 16],
    );

    let installed = vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    assert_eq!(installed.revision(), 1);
    assert_eq!(installed.group_count(), 1);
    assert_eq!(installed.entry_count(), 1);
    assert!(installed.changed());

    let ciphertext = std::fs::read(vault.path()).unwrap();
    for plaintext in [
        b"space-a".as_slice(),
        b"group-a".as_slice(),
        b"key-a".as_slice(),
        &[0x31; 32],
    ] {
        assert!(!ciphertext
            .windows(plaintext.len())
            .any(|window| window == plaintext));
    }

    let reopened =
        ProfileContentKeyVault::new(directory.path().join("vault"), secure_storage, [0x11; 16]);
    let resolved = reopened
        .resolve(
            &ContentKeyId::from_string("key-a").unwrap(),
            GroupEpoch::new(7),
        )
        .await
        .unwrap();
    assert_eq!(resolved.protection_group_id().as_str(), "group-a");
    assert_eq!(resolved.content_key_id().as_str(), "key-a");
    assert_eq!(resolved.epoch(), GroupEpoch::new(7));
    assert_eq!(resolved.key().as_bytes(), &[0x31; 32]);
    assert!(!format!("{resolved:?}").contains("key-a"));
    assert!(!format!("{resolved:?}").contains("group-a"));
}

#[tokio::test]
async fn repeated_install_is_byte_stable_and_a_second_group_merges_once() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let vault =
        ProfileContentKeyVault::new(directory.path().join("vault"), secure_storage, [0x12; 16]);
    let first = ready_material("space-a", "group-a", "key-a", 7, 0x31);
    let second = ready_material("space-b", "group-b", "key-b", 9, 0x41);

    let initial = vault.install_verified_space_material(&first).await.unwrap();
    let initial_bytes = std::fs::read(vault.path()).unwrap();
    let replay = vault.install_verified_space_material(&first).await.unwrap();
    assert_eq!(replay.revision(), initial.revision());
    assert!(!replay.changed());
    assert_eq!(std::fs::read(vault.path()).unwrap(), initial_bytes);

    let merged = vault
        .install_verified_space_material(&second)
        .await
        .unwrap();
    assert_eq!(merged.revision(), 2);
    assert_eq!(merged.group_count(), 2);
    assert_eq!(merged.entry_count(), 2);
    assert!(merged.changed());

    for (key_id, epoch, expected) in [("key-a", 7, 0x31), ("key-b", 9, 0x41)] {
        let resolved = vault
            .resolve(
                &ContentKeyId::from_string(key_id).unwrap(),
                GroupEpoch::new(epoch),
            )
            .await
            .unwrap();
        assert_eq!(resolved.key().as_bytes(), &[expected; 32]);
    }

    let merged_bytes = std::fs::read(vault.path()).unwrap();
    let second_replay = vault
        .install_verified_space_material(&second)
        .await
        .unwrap();
    assert_eq!(second_replay.revision(), 2);
    assert!(!second_replay.changed());
    assert_eq!(std::fs::read(vault.path()).unwrap(), merged_bytes);
}

#[tokio::test]
async fn conflicting_key_identity_never_changes_the_existing_vault() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let vault =
        ProfileContentKeyVault::new(directory.path().join("vault"), secure_storage, [0x13; 16]);
    vault
        .install_verified_space_material(&ready_material(
            "space-a",
            "group-a",
            "shared-key",
            7,
            0x31,
        ))
        .await
        .unwrap();
    let original = std::fs::read(vault.path()).unwrap();

    for conflict in [
        ready_material("space-a", "group-a", "shared-key", 8, 0x41),
        ready_material("space-b", "group-b", "shared-key", 7, 0x31),
    ] {
        assert!(matches!(
            vault.install_verified_space_material(&conflict).await,
            Err(ProfileContentKeyVaultError::Conflict)
        ));
        assert_eq!(std::fs::read(vault.path()).unwrap(), original);
    }

    assert!(matches!(
        vault
            .resolve(
                &ContentKeyId::from_string("shared-key").unwrap(),
                GroupEpoch::new(8),
            )
            .await,
        Err(ProfileContentKeyVaultError::EpochMismatch)
    ));
    assert!(matches!(
        vault
            .resolve(
                &ContentKeyId::from_string("missing-key").unwrap(),
                GroupEpoch::new(7),
            )
            .await,
        Err(ProfileContentKeyVaultError::KeyNotFound)
    ));
}

#[tokio::test]
async fn missing_key_and_tampered_ciphertext_fail_closed_with_sources() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let vault = ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage.clone(),
        [0x14; 16],
    );
    vault
        .install_verified_space_material(&ready_material("space-a", "group-a", "key-a", 7, 0x31))
        .await
        .unwrap();
    secure_storage
        .delete(PROFILE_CONTENT_VAULT_KEY_NAME)
        .unwrap();

    let missing_key = vault
        .resolve(
            &ContentKeyId::from_string("key-a").unwrap(),
            GroupEpoch::new(7),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        missing_key,
        ProfileContentKeyVaultError::Corrupt { .. }
    ));
    assert!(missing_key.source().is_some());
    assert!(secure_storage
        .get(PROFILE_CONTENT_VAULT_KEY_NAME)
        .unwrap()
        .is_none());

    let other_directory = tempfile::tempdir().unwrap();
    let other_storage = Arc::new(MemorySecureStorage::default());
    let other = ProfileContentKeyVault::new(
        other_directory.path().join("vault"),
        other_storage,
        [0x15; 16],
    );
    other
        .install_verified_space_material(&ready_material("space-b", "group-b", "key-b", 9, 0x41))
        .await
        .unwrap();
    let mut ciphertext = std::fs::read(other.path()).unwrap();
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 0x01;
    std::fs::write(other.path(), ciphertext).unwrap();

    let tampered = other
        .resolve(
            &ContentKeyId::from_string("key-b").unwrap(),
            GroupEpoch::new(9),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        tampered,
        ProfileContentKeyVaultError::Corrupt { .. }
    ));
    assert!(tampered.source().is_some());
}

#[tokio::test]
async fn unknown_encrypted_framing_is_rejected_before_open() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let vault =
        ProfileContentKeyVault::new(directory.path().join("vault"), secure_storage, [0x16; 16]);
    vault
        .install_verified_space_material(&ready_material("space-a", "group-a", "key-a", 7, 0x31))
        .await
        .unwrap();
    let mut framing: serde_json::Value =
        serde_json::from_slice(&std::fs::read(vault.path()).unwrap()).unwrap();
    framing["version"] = serde_json::Value::String("V2".to_owned());
    std::fs::write(vault.path(), serde_json::to_vec(&framing).unwrap()).unwrap();

    let error = vault
        .resolve(
            &ContentKeyId::from_string("key-a").unwrap(),
            GroupEpoch::new(7),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProfileContentKeyVaultError::Corrupt { .. }));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn oversized_encrypted_vault_is_rejected_before_decode() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let vault =
        ProfileContentKeyVault::new(directory.path().join("vault"), secure_storage, [0x17; 16]);
    vault
        .install_verified_space_material(&ready_material("space-a", "group-a", "key-a", 7, 0x31))
        .await
        .unwrap();
    std::fs::write(vault.path(), vec![0; 9 * 1024 * 1024]).unwrap();

    assert!(matches!(
        vault
            .resolve(
                &ContentKeyId::from_string("key-a").unwrap(),
                GroupEpoch::new(7),
            )
            .await,
        Err(ProfileContentKeyVaultError::CapacityExceeded)
    ));
}
