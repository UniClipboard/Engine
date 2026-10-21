use super::{database, input, DieselFileTransferRepository};
use crate::db::models::FileTransferRow;
use crate::db::ports::DbExecutor;
use crate::db::repositories::DieselDirectoryPublishLogRepository;
use crate::db::schema::file_transfer;
use diesel::prelude::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uc_application::deps::{BeginReceiveRetryPort, ReceiveRetryItemState};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::{
    GetDirectoryPublishRecordPort, GetEntryReceiveProgressPort, SecureStorageError,
    SecureStoragePort,
};
use uc_infra_security::{ContentProtection, InMemorySession, MasterKey, ProfileContentKeyVault};

#[derive(Default)]
struct Storage(Mutex<BTreeMap<String, Vec<u8>>>);
impl SecureStoragePort for Storage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[tokio::test]
async fn v3_retry_uses_existing_content_and_directory_envelopes() {
    let (_legacy, executor, directory) = database().await;
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        Arc::new(Storage::default()),
        [0x31; 16],
    ));
    let state = SpaceKeyState::ready_for_admission(
        SpaceId::from_str("space"),
        GroupEpoch::new(1),
        ContentKeyId::from_string("key").unwrap(),
        ProtectionGroupId::from_string("group").unwrap(),
    )
    .unwrap();
    let material = SpaceKeyMaterial::new(state, b"verified".to_vec(), serde_json::to_vec(&serde_json::json!({
        "version":2, "entries":[{"content_key_id":"legacy-v1", "epoch":0, "key":vec![0x20;32]}, {"content_key_id":"key", "epoch":1, "key":vec![0x41;32]}]
    })).unwrap(), 1);
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    session.set_master_key_for_space(
        material.state().space_id().clone(),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(&material).unwrap();
    let protection = Arc::new(ContentProtection::for_content(session, vault));
    let repo = DieselFileTransferRepository::new_v3(executor.clone(), protection.clone());
    let mut command = input();
    command.directory = true;
    for item in &mut command.items {
        item.state = ReceiveRetryItemState::Pending;
    }
    repo.begin_receive_retry(command).await.unwrap();
    let rows = executor
        .run(|conn| {
            file_transfer::table
                .load::<FileTransferRow>(conn)
                .map_err(Into::into)
        })
        .unwrap();
    for row in rows {
        let cipher = repo.protection.resolve().await.unwrap();
        let metadata = cipher
            .open_metadata(&row.transfer_id, &row.metadata_ciphertext)
            .await
            .unwrap();
        assert_eq!(metadata.filename, "PRIVATE_ORIGINAL_FILENAME");
        assert!(metadata.cached_path.is_none());
        assert!(!row
            .metadata_ciphertext
            .windows(b"PRIVATE_ORIGINAL_FILENAME".len())
            .any(|part| part == b"PRIVATE_ORIGINAL_FILENAME"));
    }
    let directory_log =
        DieselDirectoryPublishLogRepository::new_v3(executor.clone(), protection.clone());
    assert!(directory_log
        .get_publish_record("entry", "new")
        .await
        .unwrap()
        .unwrap()
        .root_map
        .is_empty());
    drop(repo);
    let reopened = DieselFileTransferRepository::new_v3(executor, protection);
    assert_eq!(
        reopened
            .get_entry_receive_progress("entry")
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1
    );
}
