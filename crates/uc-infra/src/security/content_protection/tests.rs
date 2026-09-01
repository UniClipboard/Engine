use std::collections::BTreeMap;
use std::error::Error as _;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{ContentProtection, ContentProtectionError};
use crate::security::{MasterKey, ProfileContentKeyVault};
use crate::space::InMemorySession;

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
async fn content_v3_round_trips_without_exposing_group_space_or_purpose_in_the_header() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x31; 16],
    ));
    let material = ready_material("space-a", "group-a", "key-a", 7, 0x41);
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    session.set_master_key_for_space(
        SpaceId::from_str("space-a"),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(&material).unwrap();
    let protection = ContentProtection::for_content(session, vault);

    let ciphertext = protection
        .seal_for_active(
            &Plaintext::new(b"protected payload".to_vec()),
            &Aad::new(b"entity-aad".to_vec()),
        )
        .await
        .unwrap();
    let header: serde_json::Value = serde_json::from_slice(ciphertext.as_bytes()).unwrap();
    assert_eq!(header["version"], 3);
    assert_eq!(header["aead"], "XChaCha20Poly1305");
    assert_eq!(header["content_key_id"], "key-a");
    assert_eq!(header["group_epoch"], 7);
    for forbidden in ["protection_group_id", "space_id", "purpose"] {
        assert!(header.get(forbidden).is_none());
    }

    let opened = protection
        .open(&ciphertext, &Aad::new(b"entity-aad".to_vec()))
        .await
        .unwrap();
    assert_eq!(opened.as_bytes(), b"protected payload");
}

#[tokio::test]
async fn old_v3_ciphertext_opens_after_the_active_space_switches() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x32; 16],
    ));
    let material_a = ready_material("space-a", "group-a", "key-a", 7, 0x41);
    let material_b = ready_material("space-b", "group-b", "key-b", 11, 0x42);
    vault
        .install_verified_space_material(&material_a)
        .await
        .unwrap();
    vault
        .install_verified_space_material(&material_b)
        .await
        .unwrap();
    session.set_master_key_for_space(
        SpaceId::from_str("space-a"),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(&material_a).unwrap();
    let protection = ContentProtection::for_content(session.clone(), vault);
    let aad = Aad::new(b"entity-aad".to_vec());
    let old_ciphertext = protection
        .seal_for_active(&Plaintext::new(b"space-a history".to_vec()), &aad)
        .await
        .unwrap();

    session.set_master_key_for_space(
        SpaceId::from_str("space-b"),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(&material_b).unwrap();
    let new_ciphertext = protection
        .seal_for_active(&Plaintext::new(b"space-b history".to_vec()), &aad)
        .await
        .unwrap();

    let old_header: serde_json::Value = serde_json::from_slice(old_ciphertext.as_bytes()).unwrap();
    let new_header: serde_json::Value = serde_json::from_slice(new_ciphertext.as_bytes()).unwrap();
    assert_eq!(old_header["content_key_id"], "key-a");
    assert_eq!(new_header["content_key_id"], "key-b");
    let opened = protection.open(&old_ciphertext, &aad).await.unwrap();
    assert_eq!(opened.as_bytes(), b"space-a history");
}

#[tokio::test]
async fn fixed_purpose_prevents_content_ciphertext_from_being_opened_as_search_data() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x33; 16],
    ));
    let material = ready_material("space-a", "group-a", "key-a", 7, 0x41);
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    session.set_master_key_for_space(
        SpaceId::from_str("space-a"),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(&material).unwrap();
    let content = ContentProtection::for_content(session.clone(), vault.clone());
    let search = ContentProtection::for_search(session, vault);
    let aad = Aad::new(b"entity-aad".to_vec());
    let ciphertext = content
        .seal_for_active(&Plaintext::new(b"protected payload".to_vec()), &aad)
        .await
        .unwrap();

    let error = search.open(&ciphertext, &aad).await.unwrap_err();

    assert!(matches!(
        error,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn failures_keep_stable_classification_and_a_source() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x34; 16],
    ));
    let protection = ContentProtection::for_content(session.clone(), vault.clone());
    let aad = Aad::new(b"entity-aad".to_vec());

    let not_active = protection
        .seal_for_active(&Plaintext::new(b"payload".to_vec()), &aad)
        .await
        .unwrap_err();
    assert!(matches!(
        not_active,
        ContentProtectionError::NotActive { .. }
    ));
    assert!(not_active.source().is_some());

    session.set_master_key_for_space(
        SpaceId::from_str("space-a"),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    let invalid_active = protection
        .seal_for_active(&Plaintext::new(b"payload".to_vec()), &aad)
        .await
        .unwrap_err();
    assert!(matches!(
        invalid_active,
        ContentProtectionError::InvalidActiveContext { .. }
    ));
    assert!(invalid_active.source().is_some());

    let material = ready_material("space-a", "group-a", "key-a", 7, 0x41);
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    session.install_space_material(&material).unwrap();
    let ciphertext = protection
        .seal_for_active(&Plaintext::new(b"payload".to_vec()), &aad)
        .await
        .unwrap();

    let wrong_aad = protection
        .open(&ciphertext, &Aad::new(b"different-aad".to_vec()))
        .await
        .unwrap_err();
    assert!(matches!(
        wrong_aad,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(wrong_aad.source().is_some());

    let mut wrong_version: serde_json::Value =
        serde_json::from_slice(ciphertext.as_bytes()).unwrap();
    wrong_version["version"] = serde_json::json!(4);
    let wrong_version = Ciphertext::new(serde_json::to_vec(&wrong_version).unwrap());
    let invalid = protection.open(&wrong_version, &aad).await.unwrap_err();
    assert!(matches!(
        invalid,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(invalid.source().is_some());

    let mut unknown_field: serde_json::Value =
        serde_json::from_slice(ciphertext.as_bytes()).unwrap();
    unknown_field["protection_group_id"] = serde_json::json!("group-a");
    let unknown_field = Ciphertext::new(serde_json::to_vec(&unknown_field).unwrap());
    let invalid = protection.open(&unknown_field, &aad).await.unwrap_err();
    assert!(matches!(
        invalid,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(invalid.source().is_some());

    let mut tampered: serde_json::Value = serde_json::from_slice(ciphertext.as_bytes()).unwrap();
    let first_byte = tampered["ciphertext"][0].as_u64().unwrap();
    tampered["ciphertext"][0] = serde_json::json!(first_byte ^ 1);
    let tampered = Ciphertext::new(serde_json::to_vec(&tampered).unwrap());
    let invalid = protection.open(&tampered, &aad).await.unwrap_err();
    assert!(matches!(
        invalid,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(invalid.source().is_some());

    let mut wrong_epoch: serde_json::Value = serde_json::from_slice(ciphertext.as_bytes()).unwrap();
    wrong_epoch["group_epoch"] = serde_json::json!(8);
    let wrong_epoch = Ciphertext::new(serde_json::to_vec(&wrong_epoch).unwrap());
    let unavailable = protection.open(&wrong_epoch, &aad).await.unwrap_err();
    assert!(matches!(
        unavailable,
        ContentProtectionError::KeyUnavailable { .. }
    ));
    assert!(unavailable.source().is_some());
}
