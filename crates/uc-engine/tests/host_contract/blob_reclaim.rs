use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use diesel::connection::SimpleConnection;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use diesel::{Connection, QueryableByName, RunQueryDsl};
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput, Operation, OperationResult,
    SecretString, SendImageInput,
};

use super::{startup::host, MemorySecureStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_the_only_image_reference_removes_the_blob_file_and_row() {
    let (root, engine) = start_engine().await;
    let entry_id = send_png(&engine).await;
    let (blob_id, storage_path) = wait_ready_blob(root.path(), &entry_id).await;
    let blob_file = PathBuf::from(&storage_path);
    // QueryStorageStats.vault_bytes 只统计密钥 vault 目录。
    // 内容 blob 在 profile-data-generations/.../v3-payloads/blobs，不在该目录里。
    let store_dir = blob_file.parent().expect("blob store dir").to_path_buf();
    let before = dir_size(&store_dir);
    assert!(blob_file.is_file());
    assert!(before > 0);
    age_claim(root.path(), &blob_id);
    delete_entry(&engine, &entry_id).await;
    assert_eq!(blob_count(root.path(), &blob_id), 0);
    assert!(!blob_file.is_file());
    assert!(dir_size(&store_dir) < before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_blob_is_reclaimed_only_when_the_last_referencer_is_deleted() {
    let (root, engine) = start_engine().await;
    let entry_id = send_png(&engine).await;
    let (blob_id, storage_path) = wait_ready_blob(root.path(), &entry_id).await;
    insert_second_entry(root.path(), &blob_id);
    delete_entry(&engine, &entry_id).await;
    assert_eq!(blob_count(root.path(), &blob_id), 1);
    assert!(Path::new(&storage_path).is_file());
    age_claim(root.path(), &blob_id);
    delete_entry(&engine, "entry-share").await;
    assert_eq!(blob_count(root.path(), &blob_id), 0);
    assert!(!Path::new(&storage_path).is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumbnail_blob_is_reclaimed_only_after_its_last_reference_is_gone() {
    let (root, engine) = start_engine().await;
    let entry_id = send_png(&engine).await;
    let (image_blob, image_path) = wait_ready_blob(root.path(), &entry_id).await;
    let representation_id = representation_id(root.path(), &entry_id, &image_blob);
    let thumb_id = "thumb-blob".to_string();
    let thumb_path = Path::new(&image_path)
        .parent()
        .expect("vault dir")
        .join(&thumb_id);
    std::fs::write(&thumb_path, b"thumb").unwrap();
    insert_thumbnail_blob(root.path(), &thumb_path, &representation_id, &thumb_id);
    age_claim(root.path(), &image_blob);
    delete_entry(&engine, &entry_id).await;
    assert_eq!(blob_count(root.path(), &image_blob), 0);
    assert!(!Path::new(&image_path).is_file());
    assert_eq!(blob_count(root.path(), &thumb_id), 1);
    assert!(thumb_path.is_file());
    age_claim(root.path(), &thumb_id);
    delete_entry(&engine, "entry-thumb-share").await;
    assert_eq!(blob_count(root.path(), &thumb_id), 0);
    assert!(!thumb_path.is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_recently_claimed_blob_keeps_the_row_and_file() {
    let (root, engine) = start_engine().await;
    let entry_id = send_png(&engine).await;
    let (blob_id, storage_path) = wait_ready_blob(root.path(), &entry_id).await;
    delete_entry(&engine, &entry_id).await;
    assert_eq!(blob_count(root.path(), &blob_id), 1);
    assert!(Path::new(&storage_path).is_file());
}

async fn start_engine() -> (tempfile::TempDir, Engine) {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("blob reclaim".into()),
            passphrase: SecretString::new("blob-reclaim-passphrase"),
            passphrase_confirmation: SecretString::new("blob-reclaim-passphrase"),
        }))
        .await
        .unwrap();
    (root, engine)
}

async fn send_png(engine: &Engine) -> String {
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
        panic!("image not saved");
    };
    sent.entry_id
}

fn dir_size(path: &Path) -> u64 {
    fn walk(path: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path)
                } else {
                    entry.metadata().map(|meta| meta.len()).unwrap_or(0)
                }
            })
            .sum()
    }
    walk(path)
}

async fn delete_entry(engine: &Engine, entry_id: &str) {
    let OperationResult::HistoryEntryDeleted = engine
        .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
            entry_id: entry_id.to_string(),
        }))
        .await
        .unwrap()
    else {
        panic!("entry not deleted");
    };
}

async fn wait_ready_blob(root: &Path, entry_id: &str) -> (String, String) {
    let started = Instant::now();
    loop {
        if let Some(found) = ready_blob(root, entry_id) {
            return found;
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!("image blob was not ready");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn open_db(root: &Path) -> SqliteConnection {
    let database = super::startup::runtime_database(
        root,
        "profile-data-generations",
        "v3-payloads/profile.sqlite",
    );
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    connection
        .batch_execute("PRAGMA busy_timeout = 5000;")
        .unwrap();
    connection
}

#[derive(QueryableByName)]
struct BlobPath {
    #[diesel(sql_type = Text)]
    blob_id: String,
    #[diesel(sql_type = Text)]
    storage_path: String,
}

#[derive(QueryableByName)]
struct IdRow {
    #[diesel(sql_type = Text)]
    id: String,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

fn ready_blob(root: &Path, entry_id: &str) -> Option<(String, String)> {
    let mut connection = open_db(root);
    diesel::sql_query(
        "SELECT blob.blob_id AS blob_id, blob.storage_path AS storage_path \
         FROM clipboard_entry \
         JOIN clipboard_snapshot_representation \
           ON clipboard_snapshot_representation.event_id = clipboard_entry.event_id \
         JOIN blob ON blob.blob_id = clipboard_snapshot_representation.blob_id \
         WHERE clipboard_entry.entry_id = ? AND payload_state = 'BlobReady' \
         LIMIT 1",
    )
    .bind::<Text, _>(entry_id)
    .get_result::<BlobPath>(&mut connection)
    .ok()
    .map(|row| (row.blob_id, row.storage_path))
}

fn age_claim(root: &Path, blob_id: &str) {
    let mut connection = open_db(root);
    diesel::sql_query("UPDATE blob SET last_claimed_at_ms = 0 WHERE blob_id = ?")
        .bind::<Text, _>(blob_id)
        .execute(&mut connection)
        .unwrap();
}

fn blob_count(root: &Path, blob_id: &str) -> i64 {
    let mut connection = open_db(root);
    diesel::sql_query("SELECT COUNT(*) AS n FROM blob WHERE blob_id = ?")
        .bind::<Text, _>(blob_id)
        .get_result::<CountRow>(&mut connection)
        .unwrap()
        .n
}

fn representation_id(root: &Path, entry_id: &str, blob_id: &str) -> String {
    let mut connection = open_db(root);
    diesel::sql_query(
        "SELECT clipboard_snapshot_representation.id AS id \
         FROM clipboard_entry \
         JOIN clipboard_snapshot_representation \
           ON clipboard_snapshot_representation.event_id = clipboard_entry.event_id \
         WHERE clipboard_entry.entry_id = ? \
           AND clipboard_snapshot_representation.blob_id = ?",
    )
    .bind::<Text, _>(entry_id)
    .bind::<Text, _>(blob_id)
    .get_result::<IdRow>(&mut connection)
    .unwrap()
    .id
}

fn insert_second_entry(root: &Path, blob_id: &str) {
    let mut connection = open_db(root);
    diesel::sql_query(
        "INSERT INTO clipboard_event (event_id, captured_at_ms, source_device, snapshot_hash) \
         VALUES ('ev-share', 2, 'device', 'snap-share')",
    )
    .execute(&mut connection)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_entry (entry_id, event_id, created_at_ms, active_time_ms, \
         total_size, pinned, delivery_tracked, is_favorited, content_category) \
         VALUES ('entry-share', 'ev-share', 2, 2, 8, 0, 0, 0, 'image')",
    )
    .execute(&mut connection)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_snapshot_representation \
         (id, event_id, format_id, mime_type, size_bytes, blob_id, payload_state) \
         VALUES ('rep-share', 'ev-share', 'public.png', 'image/png', 8, ?, 'BlobReady')",
    )
    .bind::<Text, _>(blob_id)
    .execute(&mut connection)
    .unwrap();
}

fn insert_thumbnail_blob(root: &Path, path: &PathBuf, representation_id: &str, thumb_id: &str) {
    let mut connection = open_db(root);
    let hash = format!("blake3v1:{}", "ab".repeat(32));
    diesel::sql_query(
        "INSERT INTO blob (blob_id, storage_path, storage_backend, size_bytes, content_hash, \
         created_at_ms, compressed_size, last_claimed_at_ms) \
         VALUES (?, ?, 'local_fs', 5, ?, 3, 5, 3)",
    )
    .bind::<Text, _>(thumb_id)
    .bind::<Text, _>(path.to_str().unwrap())
    .bind::<Text, _>(&hash)
    .execute(&mut connection)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_representation_thumbnail \
         (representation_id, thumbnail_blob_id, thumbnail_mime_type, original_width, \
          original_height, original_size_bytes) \
         VALUES (?, ?, 'image/webp', 1, 1, 5)",
    )
    .bind::<Text, _>(representation_id)
    .bind::<Text, _>(thumb_id)
    .execute(&mut connection)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_event (event_id, captured_at_ms, source_device, snapshot_hash) \
         VALUES ('ev-thumb', 3, 'device', 'snap-thumb')",
    )
    .execute(&mut connection)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_entry (entry_id, event_id, created_at_ms, active_time_ms, \
         total_size, pinned, delivery_tracked, is_favorited, content_category) \
         VALUES ('entry-thumb-share', 'ev-thumb', 3, 3, 5, 0, 0, 0, 'image')",
    )
    .execute(&mut connection)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_snapshot_representation \
         (id, event_id, format_id, mime_type, size_bytes, blob_id, payload_state) \
         VALUES ('rep-thumb-share', 'ev-thumb', 'public.png', 'image/png', 5, ?, 'BlobReady')",
    )
    .bind::<Text, _>(thumb_id)
    .execute(&mut connection)
    .unwrap();
}
