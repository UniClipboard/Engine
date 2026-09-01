use std::collections::BTreeMap;
use std::error::Error as _;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uc_core::blob::ports::BlobReaderPort;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::security::{BlobCipherError, BlobCipherPort};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_core::BlobId;

use super::{
    envelope, ContentProtection, ContentProtectionError, V3EncryptedBlobStore,
    V3InlinePayloadCipher,
};
use crate::blob::{BlobStorePort, FilesystemBlobStore};
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
    assert_eq!(&ciphertext.as_bytes()[..4], b"UCP3");
    assert_eq!(
        u16::from_be_bytes(ciphertext.as_bytes()[4..6].try_into().unwrap()),
        3
    );
    let header = envelope::decode(ciphertext.as_bytes()).unwrap();
    assert_eq!(header.content_key_id.as_str(), "key-a");
    assert_eq!(header.group_epoch, GroupEpoch::new(7));
    assert!(
        ciphertext.len() < 128,
        "small payload framing must stay compact"
    );
    for forbidden in [b"group-a".as_slice(), b"space-a", b"content"] {
        assert!(!ciphertext
            .as_bytes()
            .windows(forbidden.len())
            .any(|window| window == forbidden));
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

    let old_header = envelope::decode(old_ciphertext.as_bytes()).unwrap();
    let new_header = envelope::decode(new_ciphertext.as_bytes()).unwrap();
    assert_eq!(old_header.content_key_id.as_str(), "key-a");
    assert_eq!(new_header.content_key_id.as_str(), "key-b");
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

    let mut wrong_version = ciphertext.as_bytes().to_vec();
    wrong_version[5] = 4;
    let wrong_version = Ciphertext::new(wrong_version);
    let invalid = protection.open(&wrong_version, &aad).await.unwrap_err();
    assert!(matches!(
        invalid,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(invalid.source().is_some());

    let mut unknown_algorithm = ciphertext.as_bytes().to_vec();
    unknown_algorithm[6] = 2;
    let invalid = protection
        .open(&Ciphertext::new(unknown_algorithm), &aad)
        .await
        .unwrap_err();
    assert!(matches!(
        invalid,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(invalid.source().is_some());

    let truncated = protection
        .open(&Ciphertext::new(b"UCP3".to_vec()), &aad)
        .await
        .unwrap_err();
    assert!(matches!(
        truncated,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(truncated.source().is_some());

    let mut tampered = ciphertext.as_bytes().to_vec();
    *tampered.last_mut().unwrap() ^= 1;
    let tampered = Ciphertext::new(tampered);
    let invalid = protection.open(&tampered, &aad).await.unwrap_err();
    assert!(matches!(
        invalid,
        ContentProtectionError::InvalidCiphertext { .. }
    ));
    assert!(invalid.source().is_some());

    let mut wrong_epoch = ciphertext.as_bytes().to_vec();
    wrong_epoch[16] ^= 1;
    let wrong_epoch = Ciphertext::new(wrong_epoch);
    let unavailable = protection.open(&wrong_epoch, &aad).await.unwrap_err();
    assert!(matches!(
        unavailable,
        ContentProtectionError::KeyUnavailable { .. }
    ));
    assert!(unavailable.source().is_some());
}

#[tokio::test]
async fn inline_and_ucbl_v3_remain_readable_after_the_active_space_switches() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x81; 16],
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

    let protection = Arc::new(ContentProtection::for_content(
        Arc::clone(&session),
        Arc::clone(&vault),
    ));
    let inline = V3InlinePayloadCipher::new(Arc::clone(&protection));
    let inner: Arc<dyn BlobStorePort> =
        Arc::new(FilesystemBlobStore::new(directory.path().join("blobs")));
    let blobs = V3EncryptedBlobStore::new(Arc::clone(&inner), protection);
    let inline_aad = Aad::new(b"inline-entity".to_vec());
    let inline_ciphertext = inline
        .encrypt(&Plaintext::new(b"history-inline".to_vec()), &inline_aad)
        .await
        .unwrap();
    let blob_id = BlobId::from("history-blob");
    let (blob_path, _) = blobs.put(&blob_id, b"history-blob-payload").await.unwrap();

    for bytes in [
        inline_ciphertext.as_bytes().to_vec(),
        tokio::fs::read(&blob_path).await.unwrap(),
    ] {
        for forbidden in [
            b"history-inline".as_slice(),
            b"history-blob-payload",
            b"space-a",
            b"group-a",
            b"content",
        ] {
            assert!(!bytes
                .windows(forbidden.len())
                .any(|window| window == forbidden));
        }
    }
    let raw_blob = tokio::fs::read(blob_path).await.unwrap();
    assert_eq!(&raw_blob[..4], b"UCBL");
    assert_eq!(raw_blob[4], 3);
    let transplanted_blob_id = BlobId::from("transplanted-blob");
    inner.put(&transplanted_blob_id, &raw_blob).await.unwrap();

    session.set_master_key_for_space(
        SpaceId::from_str("space-b"),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(&material_b).unwrap();

    assert_eq!(
        inline
            .decrypt(&inline_ciphertext, &inline_aad)
            .await
            .unwrap()
            .as_bytes(),
        b"history-inline"
    );
    assert_eq!(
        BlobReaderPort::get(&blobs, &blob_id).await.unwrap(),
        b"history-blob-payload"
    );
    assert!(BlobReaderPort::get(&blobs, &transplanted_blob_id)
        .await
        .is_err());
}

#[tokio::test]
async fn inline_v3_errors_keep_stable_classification_and_source() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x82; 16],
    ));
    let inline =
        V3InlinePayloadCipher::new(Arc::new(ContentProtection::for_content(session, vault)));

    let error = inline
        .encrypt(
            &Plaintext::new(b"payload".to_vec()),
            &Aad::new(b"entity".to_vec()),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, BlobCipherError::NotUnlocked { .. }));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn ucbl_v3_path_ingest_preserves_plaintext_identity_and_delete_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x83; 16],
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
    let inner: Arc<dyn BlobStorePort> =
        Arc::new(FilesystemBlobStore::new(directory.path().join("blobs")));
    let blobs = V3EncryptedBlobStore::new(
        inner,
        Arc::new(ContentProtection::for_content(session, vault)),
    );
    let source = directory.path().join("source");
    let payload = b"path-backed V3 profile payload";
    tokio::fs::write(&source, payload).await.unwrap();
    let blob_id = BlobId::from("path-blob");

    let stored = blobs.put_from_path(&blob_id, &source).await.unwrap();

    assert_eq!(stored.size_bytes, payload.len() as u64);
    assert_eq!(
        stored.content_hash,
        uc_core::ContentHash::from(blake3::hash(payload).as_bytes())
    );
    assert!(stored.compressed_size.is_some());
    assert_eq!(
        BlobReaderPort::get(&blobs, &blob_id).await.unwrap(),
        payload
    );

    blobs.delete(&blob_id).await.unwrap();
    assert!(BlobReaderPort::get(&blobs, &blob_id).await.is_err());
    blobs.delete(&blob_id).await.unwrap();
}
