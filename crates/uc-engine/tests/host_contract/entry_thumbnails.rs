use std::time::Duration;

use diesel::sqlite::SqliteConnection;
use diesel::{connection::SimpleConnection, Connection, RunQueryDsl};
use sha2::{Digest, Sha256};
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput, Operation, OperationResult,
    SecretString, SendImageInput, SendTextInput,
};

use super::{startup::host, MemorySecureStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entry_thumbnails_use_public_identity_and_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("thumbnail acceptance".into()),
            passphrase: SecretString::new("thumbnail-acceptance-passphrase"),
            passphrase_confirmation: SecretString::new("thumbnail-acceptance-passphrase"),
        }))
        .await
        .unwrap();
    let original =
        include_bytes!("../../../../tests/hosts/connectivity/tests/fixtures/active-clipboard.png");
    let OperationResult::EntrySent(sent) = engine
        .execute(Operation::SendImage(SendImageInput {
            bytes: original.to_vec(),
            mime_type: "image/png".into(),
            target_devices: vec![],
        }))
        .await
        .unwrap()
    else {
        panic!("image not saved")
    };
    let entry = HistoryEntryInput {
        entry_id: sent.entry_id.clone(),
    };
    let started = std::time::Instant::now();
    let mut initial_busy = 0;
    let first = loop {
        match engine
            .execute(Operation::ReadEntryThumbnail(entry.clone()))
            .await
        {
            Ok(OperationResult::ThumbnailRead(value)) => break value,
            Err(error) if error.code() == 1465 && started.elapsed() < Duration::from_secs(10) => {
                assert!(error.is_retryable());
                initial_busy += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            other => panic!("thumbnail read did not converge: {other:?}"),
        }
    };
    let initial_elapsed_ms = started.elapsed().as_millis();
    assert_eq!(first.media_type.as_deref(), Some("image/webp"));
    assert_eq!(&first.bytes[8..12], b"WEBP");
    assert_eq!(&first.bytes[12..16], b"VP8L");
    assert_eq!(first.bytes[20], 0x2f);
    let dimensions = u32::from_le_bytes(first.bytes[21..25].try_into().unwrap());
    let thumbnail_width = (dimensions & 0x3fff) + 1;
    let thumbnail_height = ((dimensions >> 14) & 0x3fff) + 1;
    assert_eq!((thumbnail_width, thumbnail_height), (128, 85));
    let (a, b, c) = tokio::join!(
        engine.execute(Operation::ReadEntryThumbnail(entry.clone())),
        engine.execute(Operation::ReadEntryThumbnail(entry.clone())),
        engine.execute(Operation::ReadEntryThumbnail(entry.clone()))
    );
    for result in [a, b, c] {
        let OperationResult::ThumbnailRead(value) = result.unwrap() else {
            panic!("cached thumbnail missing")
        };
        assert_eq!(value.bytes, first.bytes);
    }
    let OperationResult::HistoryEntryResource(resource) = engine
        .execute(Operation::GetHistoryEntryResource(entry.clone()))
        .await
        .unwrap()
    else {
        panic!("original descriptor missing")
    };
    let original_bytes = if let Some(bytes) = resource.inline_data {
        bytes
    } else {
        let OperationResult::BlobRead(blob) = engine
            .execute(Operation::ReadBlob(uc_engine::BlobResourceInput {
                blob_id: resource.blob_id.unwrap(),
            }))
            .await
            .unwrap()
        else {
            panic!("original blob missing")
        };
        blob.bytes
    };
    assert_eq!(&original_bytes, original);
    let inline = include_bytes!("../fixtures/thumbnail-inline.png");
    let OperationResult::EntrySent(inline_sent) = engine
        .execute(Operation::SendImage(SendImageInput {
            bytes: inline.to_vec(),
            mime_type: "image/png".into(),
            target_devices: vec![],
        }))
        .await
        .unwrap()
    else {
        panic!("inline image not saved")
    };
    let inline_entry = HistoryEntryInput {
        entry_id: inline_sent.entry_id,
    };
    let OperationResult::HistoryEntryResource(inline_descriptor) = engine
        .execute(Operation::GetHistoryEntryResource(inline_entry.clone()))
        .await
        .unwrap()
    else {
        panic!("inline descriptor missing")
    };
    assert_eq!(
        inline_descriptor.inline_data.as_deref(),
        Some(inline.as_slice())
    );
    assert!(matches!(
        engine
            .execute(Operation::ReadEntryThumbnail(inline_entry.clone()))
            .await
            .unwrap(),
        OperationResult::ThumbnailRead(_)
    ));
    let mut failure_codes = Vec::new();
    for bytes in [
        b"controlled invalid image bytes".as_slice(),
        include_bytes!("../fixtures/thumbnail-over-dimension.png").as_slice(),
    ] {
        let OperationResult::EntrySent(saved) = engine
            .execute(Operation::SendImage(SendImageInput {
                bytes: bytes.to_vec(),
                mime_type: "image/png".into(),
                target_devices: vec![],
            }))
            .await
            .unwrap()
        else {
            panic!("failure fixture not saved")
        };
        let error = engine
            .execute(Operation::ReadEntryThumbnail(HistoryEntryInput {
                entry_id: saved.entry_id,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), 1464);
        failure_codes.push(error.code());
    }
    let OperationResult::EntrySent(text) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "thumbnail non-image".into(),
            target_devices: vec![],
        }))
        .await
        .unwrap()
    else {
        panic!("text not saved")
    };
    let unavailable = engine
        .execute(Operation::ReadEntryThumbnail(HistoryEntryInput {
            entry_id: text.entry_id,
        }))
        .await
        .unwrap_err();
    assert_eq!(unavailable.code(), 1466);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let database = super::startup::runtime_database(
        root.path(),
        "profile-data-generations",
        "v3-payloads/profile.sqlite",
    );
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    connection
        .batch_execute("DELETE FROM clipboard_representation_thumbnail")
        .unwrap();
    drop(connection);
    let (engine, _) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    let (cold_a, cold_b, cold_c) = tokio::join!(
        engine.execute(Operation::ReadEntryThumbnail(entry.clone())),
        engine.execute(Operation::ReadEntryThumbnail(entry.clone())),
        engine.execute(Operation::ReadEntryThumbnail(entry.clone()))
    );
    let mut cold_ready = 0;
    let mut cold_busy = 0;
    for result in [cold_a, cold_b, cold_c] {
        match result {
            Ok(OperationResult::ThumbnailRead(value)) => {
                assert_eq!(value.bytes, first.bytes);
                cold_ready += 1;
            }
            Err(error) => {
                assert_eq!(error.code(), 1465);
                assert!(error.is_retryable());
                cold_busy += 1;
            }
            _ => panic!("unexpected cold read result"),
        }
    }
    assert!(cold_ready > 0);
    let OperationResult::ThumbnailRead(second) = engine
        .execute(Operation::ReadEntryThumbnail(entry.clone()))
        .await
        .unwrap()
    else {
        panic!("thumbnail not returned after restart")
    };
    assert_eq!(first.bytes, second.bytes);
    assert!(matches!(
        engine
            .execute(Operation::ReadEntryThumbnail(inline_entry.clone()))
            .await
            .unwrap(),
        OperationResult::ThumbnailRead(_)
    ));
    let (deletion, racing_read) = tokio::join!(
        engine.execute(Operation::DeleteHistoryEntry(entry.clone())),
        engine.execute(Operation::ReadEntryThumbnail(entry.clone()))
    );
    deletion.unwrap();
    let race_result = match racing_read {
        Ok(OperationResult::ThumbnailRead(_)) => "ready_before_delete",
        Err(error) => {
            assert_eq!(error.code(), 1462);
            "not_found"
        }
        _ => panic!("unexpected deletion race outcome"),
    };
    let deleted = engine
        .execute(Operation::ReadEntryThumbnail(entry))
        .await
        .unwrap_err();
    assert_eq!(deleted.code(), 1462);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    diesel::sql_query("UPDATE clipboard_snapshot_representation SET inline_data = NULL, blob_id = NULL, payload_state = 'Lost' WHERE id = (SELECT preview_rep_id FROM clipboard_selection WHERE entry_id = ?)")
        .bind::<diesel::sql_types::Text, _>(&inline_entry.entry_id)
        .execute(&mut connection).unwrap();
    drop(connection);
    let (engine, _) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    let lost = engine
        .execute(Operation::ReadEntryThumbnail(inline_entry.clone()))
        .await
        .unwrap_err();
    assert_eq!(lost.code(), 1466);
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    diesel::sql_query("DELETE FROM clipboard_selection WHERE entry_id = ?")
        .bind::<diesel::sql_types::Text, _>(&inline_entry.entry_id)
        .execute(&mut connection)
        .unwrap();
    let missing_preview = engine
        .execute(Operation::ReadEntryThumbnail(inline_entry))
        .await
        .unwrap_err();
    assert_eq!(missing_preview.code(), 1466);
    drop(connection);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let output = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/test-artifacts/entry-thumbnails".into());
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("thumbnail.webp"), &first.bytes).unwrap();
    std::fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "original_sha256": format!("{:x}", Sha256::digest(original)),
            "thumbnail_sha256": format!("{:x}", Sha256::digest(&first.bytes)),
            "thumbnail_bytes": first.bytes.len(), "restart_equal": true,
            "thumbnail_width": thumbnail_width, "thumbnail_height": thumbnail_height,
            "initial_busy": initial_busy, "initial_elapsed_ms": initial_elapsed_ms,
            "non_image_code": unavailable.code(), "deleted_code": deleted.code(),
            "cached_parallel_reads": 3, "cold_ready": cold_ready, "cold_busy": cold_busy,
            "derived_metadata_removed_before_restart": true, "inline_verified": true,
            "original_bytes_unchanged": true, "invalid_and_dimension_limit_codes": failure_codes,
            "lost_source_code": lost.code(), "missing_preview_code": missing_preview.code(),
            "deletion_race": race_result, "inline_source_dimensions": [2048,1536]
        }))
        .unwrap(),
    )
    .unwrap();
}
