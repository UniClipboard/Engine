use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use diesel::RunQueryDsl as _;
use uc_core::file_transfer::{FileTransferEvent, FileTransferEventStorePort};
use uc_core::ids::{DeviceId, EntryId, SpaceId};
use uc_core::ports::clipboard::EntryFileSetRepositoryPort;
use uc_core::ports::{
    AdvanceActiveClipboardPort, GetDirectoryPublishRecordPort, ListProvisionalReceivesPort,
    ListUnsettledReceiveArtifactsPort, LoadMobileConsumableClipboardPort, PublishPhase,
    ReceiveArtifact, ReceiveArtifactOwnership, ReceiveArtifactPhase, ReceiveArtifactRecord,
    ReceiveArtifactResolution, RecordDirectoryPublishPort, RecordReceiveArtifactsPort,
    RecordReceiverTransferPort, SecureStorageError, SecureStoragePort, SeedProvisionalReceivePort,
    UpdateProvisionalReceivePathPort,
};

use crate::db::repositories::active_clipboard_register_cipher::V3ActiveClipboardRegisterCipher;
use crate::db::repositories::directory_publish_log_cipher::V3DirectoryPublishLogCipher;
use crate::db::repositories::entry_file_set_cipher::V3EntryFileSetPathCipher;
use crate::db::repositories::receive_artifact_cipher::V3ReceiveArtifactCipher;
use crate::db::repositories::{
    DieselActiveClipboardRegisterRepository, DieselDirectoryPublishLogRepository,
    DieselEntryFileSetRepository, DieselFileTransferRepository, DieselReceiveArtifactLogRepository,
};
use crate::db::{
    executor::DieselSqliteExecutor,
    models::{NewClipboardEntryRow, NewClipboardEventRow},
    pool::init_db_pool,
    ports::DbExecutor,
    schema::{clipboard_entry, clipboard_event},
};
use crate::file_transfer::persistence_cipher::{TransferMetadata, V3TransferPersistenceCipher};
use crate::file_transfer::SqliteReceiverFileTransferStore;
use crate::security::{ContentProtection, MasterKey, ProfileContentKeyVault};
use crate::space::InMemorySession;

#[derive(Default)]
struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
        Ok(())
    }
}

async fn protection() -> (tempfile::TempDir, Arc<ContentProtection>) {
    let directory = tempfile::tempdir().unwrap();
    let session = Arc::new(InMemorySession::new());
    let space_id = SpaceId::from_str("field-codec-space");
    session.set_master_key_for_space(
        space_id.clone(),
        MasterKey::from_bytes(&[0xA1; 32]).unwrap(),
    );
    let material = session
        .create_migrated_space_material(&space_id, 1)
        .unwrap();
    session.install_space_material(&material).unwrap();
    let storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        storage,
        [0xA2; 16],
    ));
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    let protection = Arc::new(ContentProtection::for_content(session, vault));
    (directory, protection)
}

#[tokio::test]
async fn specialized_v3_codecs_keep_owner_serialization_and_entity_aad() {
    let (_directory, protection) = protection().await;
    let entry_id = EntryId::from("entry-field-codec");

    let file_set = V3EntryFileSetPathCipher::new(Arc::clone(&protection));
    let original = file_set
        .seal_original_text(&entry_id, 7, "private/original.txt")
        .await
        .unwrap();
    assert_eq!(
        file_set
            .open_original_text(&entry_id, 7, &original)
            .await
            .unwrap(),
        "private/original.txt"
    );
    assert!(file_set
        .open_relative_path(&entry_id, 7, &original)
        .await
        .is_err());

    let transfer = V3TransferPersistenceCipher::new(Arc::clone(&protection));
    let metadata = TransferMetadata {
        filename: "private-transfer.txt".to_owned(),
        cached_path: Some("/private/cache".to_owned()),
        failure_detail: None,
    };
    let sealed_metadata = transfer
        .seal_metadata("transfer-a", &metadata)
        .await
        .unwrap();
    assert_eq!(
        transfer
            .open_metadata("transfer-a", &sealed_metadata)
            .await
            .unwrap(),
        metadata
    );
    assert!(transfer
        .open_metadata("transfer-b", &sealed_metadata)
        .await
        .is_err());
    let event =
        FileTransferEvent::started("transfer-a", "peer-a", "private-transfer.txt", Some(19));
    let sealed_event = transfer
        .seal_event("transfer-a", 3, "started", &event)
        .await
        .unwrap();
    assert_eq!(
        transfer
            .open_event("transfer-a", 3, "started", &sealed_event)
            .await
            .unwrap(),
        event
    );

    let active = V3ActiveClipboardRegisterCipher::new(Arc::clone(&protection));
    let reference =
        uc_core::clipboard::MobileConsumableRef::new("private-snapshot-hash", entry_id.clone());
    let sealed_reference = active.seal(&reference).await.unwrap();
    assert_eq!(active.open(&sealed_reference).await.unwrap(), reference);

    let publish = V3DirectoryPublishLogCipher::new(Arc::clone(&protection));
    let roots = vec![(
        PathBuf::from("/private/staging"),
        PathBuf::from("/private/destination"),
    )];
    let sealed_roots = publish.seal(&entry_id, "attempt-a", &roots).await.unwrap();
    assert_eq!(
        publish
            .open(&entry_id, "attempt-a", &sealed_roots)
            .await
            .unwrap(),
        roots
    );
    assert!(publish
        .open(&entry_id, "attempt-b", &sealed_roots)
        .await
        .is_err());

    let receive = V3ReceiveArtifactCipher::new(protection);
    let artifacts = vec![ReceiveArtifact {
        item_id: "item-a".to_owned(),
        staged_path: PathBuf::from("/private/receive-staging"),
        final_path: PathBuf::from("/private/receive-final"),
        ownership: ReceiveArtifactOwnership::ManagedStaging,
    }];
    let sealed_artifacts = receive
        .seal(entry_id.as_ref(), "attempt-a", &artifacts)
        .await
        .unwrap();
    assert_eq!(
        receive
            .open(entry_id.as_ref(), "attempt-a", &sealed_artifacts)
            .await
            .unwrap(),
        artifacts
    );
    assert!(receive
        .open(entry_id.as_ref(), "attempt-b", &sealed_artifacts)
        .await
        .is_err());
}

#[tokio::test]
async fn specialized_repositories_use_only_the_selected_v3_strategy() {
    let (directory, protection) = protection().await;
    let database = directory.path().join("v3-repositories.sqlite");
    let pool = init_db_pool(database.to_str().unwrap()).unwrap();

    let active = DieselActiveClipboardRegisterRepository::new_v3(
        DieselSqliteExecutor::new(pool.clone()),
        Arc::clone(&protection),
    );
    let state = uc_core::clipboard::ActiveClipboardState::new(
        "private-v3-snapshot",
        EntryId::from("entry-v3"),
        10,
        DeviceId::new("device-v3"),
    );
    active.advance(&state, true).await.unwrap();
    assert_eq!(
        active.load_mobile_consumable().await.unwrap(),
        Some(uc_core::clipboard::MobileConsumableRef::new(
            "private-v3-snapshot",
            EntryId::from("entry-v3"),
        ))
    );

    let seed = DieselSqliteExecutor::new(pool.clone());
    seed.run(|connection| {
        diesel::insert_into(clipboard_event::table)
            .values(&NewClipboardEventRow {
                event_id: "event-v3".to_owned(),
                captured_at_ms: 10,
                source_device: "device-v3".to_owned(),
                snapshot_hash: "snapshot-v3".to_owned(),
            })
            .execute(connection)?;
        diesel::insert_into(clipboard_entry::table)
            .values(&NewClipboardEntryRow {
                entry_id: "entry-v3".to_owned(),
                event_id: "event-v3".to_owned(),
                created_at_ms: 10,
                active_time_ms: 10,
                total_size: 0,
                pinned: false,
                delivery_tracked: true,
                is_favorited: false,
                content_category: "file".to_owned(),
            })
            .execute(connection)?;
        Ok(())
    })
    .unwrap();
    let file_set = DieselEntryFileSetRepository::new_v3(
        DieselSqliteExecutor::new(pool.clone()),
        Arc::clone(&protection),
    );
    let file_set_value = uc_core::clipboard::EntryFileSet {
        lines: vec![uc_core::clipboard::EntryFileSetLine {
            line_index: 0,
            original_text: "private-file-set-original".to_owned(),
            member_location: Some(uc_core::clipboard::FileSetMemberLocation {
                root_index: 0,
                root_name: "private-root-name".to_owned(),
                relative_path: "private-relative-path".to_owned(),
                kind: uc_core::clipboard::FileSetMemberKind::File,
            }),
            kind: uc_core::clipboard::EntryFileSetLineKind::NonFile,
        }],
    };
    file_set
        .save(&EntryId::from("entry-v3"), &file_set_value)
        .await
        .unwrap();
    assert_eq!(
        file_set.load(&EntryId::from("entry-v3")).await.unwrap(),
        Some(file_set_value)
    );

    let publish = DieselDirectoryPublishLogRepository::new_v3(
        DieselSqliteExecutor::new(pool.clone()),
        Arc::clone(&protection),
    );
    let roots = vec![(
        PathBuf::from("private-stage"),
        PathBuf::from("private-final"),
    )];
    publish
        .record_phase("entry-v3", "attempt-v3", PublishPhase::Staging, &roots, 11)
        .await
        .unwrap();
    assert_eq!(
        publish
            .get_publish_record("entry-v3", "attempt-v3")
            .await
            .unwrap()
            .unwrap()
            .root_map,
        roots
    );

    let receive = DieselReceiveArtifactLogRepository::new_v3(
        DieselSqliteExecutor::new(pool.clone()),
        Arc::clone(&protection),
    );
    let record = ReceiveArtifactRecord {
        entry_id: "entry-v3".to_owned(),
        attempt_id: "attempt-v3".to_owned(),
        phase: ReceiveArtifactPhase::Preparing,
        resolution: ReceiveArtifactResolution::Pending,
        artifacts: vec![ReceiveArtifact {
            item_id: "item-v3".to_owned(),
            staged_path: PathBuf::from("private-receive-stage"),
            final_path: PathBuf::from("private-receive-final"),
            ownership: ReceiveArtifactOwnership::ManagedStaging,
        }],
        updated_at_ms: 12,
    };
    receive.record_receive_artifacts(&record).await.unwrap();
    assert_eq!(
        receive.list_unsettled_receive_artifacts().await.unwrap(),
        vec![record]
    );

    let transfers = DieselFileTransferRepository::new_v3(
        DieselSqliteExecutor::new(pool.clone()),
        Arc::clone(&protection),
    );
    transfers
        .seed_provisional_receive(&uc_core::ports::file_transfer::ProvisionalInboundTransfer {
            transfer_id: "transfer-v3".to_owned(),
            origin_device_id: "device-v3".to_owned(),
            filename: "private-transfer-name".to_owned(),
            file_size: Some(23),
            created_at_ms: 13,
        })
        .await
        .unwrap();
    transfers
        .update_provisional_receive_path("transfer-v3", "private-transfer-path", 14)
        .await
        .unwrap();
    assert_eq!(
        transfers.list_provisional_receives().await.unwrap(),
        vec![uc_core::ports::file_transfer::ProvisionalReceiveRecovery {
            transfer_id: "transfer-v3".to_owned(),
            cached_path: Some("private-transfer-path".to_owned()),
        }]
    );
    transfers
        .upsert_pending_transfer(&uc_core::ports::file_transfer::PendingInboundTransfer {
            transfer_id: "event-transfer-v3".to_owned(),
            entry_id: "entry-v3".to_owned(),
            attempt_id: None,
            origin_device_id: "device-v3".to_owned(),
            filename: "private-event-original-name".to_owned(),
            file_size: None,
            cached_path: "private-event-path".to_owned(),
            created_at_ms: 15,
        })
        .await
        .unwrap();
    let receiver = SqliteReceiverFileTransferStore::new_v3(
        DieselSqliteExecutor::new(pool),
        Arc::clone(&protection),
    );
    let started = FileTransferEvent::started(
        "event-transfer-v3",
        "peer-v3",
        "private-event-updated-name",
        Some(31),
    );
    receiver.append(started.clone()).await.unwrap();
    assert_eq!(
        receiver.load("event-transfer-v3").await.unwrap(),
        vec![started]
    );

    drop(active);
    drop(file_set);
    drop(publish);
    drop(receive);
    drop(transfers);
    drop(receiver);
    let bytes = std::fs::read(database).unwrap();
    for plaintext in [
        b"private-stage".as_slice(),
        b"private-final",
        b"private-receive-stage",
        b"private-receive-final",
        b"private-file-set-original",
        b"private-root-name",
        b"private-relative-path",
        b"private-transfer-name",
        b"private-transfer-path",
        b"private-event-original-name",
        b"private-event-updated-name",
        b"private-event-path",
    ] {
        assert!(!bytes
            .windows(plaintext.len())
            .any(|window| window == plaintext));
    }
}
