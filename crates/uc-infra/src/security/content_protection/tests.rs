//! `ContentProtection`（`uc-infra-security`）与本 crate 仍持有的
//! `ProfilePayloadAdapters`/`V3EncryptedBlobStore` 的跨 crate 集成验证。
//!
//! 这两条场景原本和 `ContentProtection` 的纯内部测试同在一个模块；security
//! 拆成独立 crate 后，`ProfilePayloadAdapters`（content/profile 未拆分前）
//! 不能再和 security 的纯测试共享模块，因此迁到这里。
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uc_core::blob::ports::BlobReaderPort;
use uc_core::crypto::domain::{Aad, Plaintext};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_core::BlobId;

use super::V3EncryptedBlobStore;
use crate::security::ProfilePayloadAdapters;
use uc_infra_crypto::secrets::MasterKey;
use uc_infra_local::blob::{BlobStorePort, FilesystemBlobStore};
use uc_infra_security::{ContentProtection, InMemorySession, ProfileContentKeyVault};

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
    let inner: Arc<dyn BlobStorePort> =
        Arc::new(FilesystemBlobStore::new(directory.path().join("blobs")));
    let adapters = ProfilePayloadAdapters::v3(Arc::clone(&inner), protection);
    let inline = adapters.inline_cipher();
    let blobs = adapters.blob_store();
    let blob_reader = adapters.blob_reader();
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
        blob_reader.get(&blob_id).await.unwrap(),
        b"history-blob-payload"
    );
    assert!(blob_reader.get(&transplanted_blob_id).await.is_err());
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
