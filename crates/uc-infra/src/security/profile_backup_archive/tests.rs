use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use diesel::connection::SimpleConnection;
use diesel::{Connection, RunQueryDsl, SqliteConnection};
use tempfile::{tempdir, TempDir};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::super::MasterKey;
use super::stream::{ArchiveReader, ArchiveWriter};
use super::{key_name, ProfileBackupArchive, ProfileBackupArchiveError, ProfileBackupSource};

#[derive(Default)]
struct MemoryStorage(Mutex<BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for MemoryStorage {
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

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    source: PathBuf,
    storage: Arc<MemoryStorage>,
    archive: ProfileBackupArchive,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let archive = ProfileBackupArchive::new(root.join("backups"), storage.clone());
        Self {
            _temporary: temporary,
            root,
            source,
            storage,
            archive,
        }
    }
}

fn source_version() -> ProfileBackupSource {
    ProfileBackupSource {
        product_version: "old-product-1.0".into(),
        engine_version: "old-engine-0.9".into(),
        platform: "macos".into(),
        architecture: "aarch64".into(),
        installation_channel: "direct".into(),
        artifact_digest: [42; 32],
    }
}

#[test]
fn cold_sqlite_wal_and_files_restore_without_schema_migration_or_source_writes() {
    let fixture = Fixture::new();
    let database = fixture.source.join("legacy.sqlite");
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    connection
        .batch_execute(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        PRAGMA user_version=73; CREATE TABLE old_history (content TEXT NOT NULL);
        INSERT INTO old_history VALUES ('pre-upgrade-private-probe');",
        )
        .unwrap();
    fs::create_dir(fixture.source.join("empty-directory")).unwrap();
    fs::create_dir(fixture.source.join("attachments")).unwrap();
    fs::write(fixture.source.join("attachments/image.bin"), [13; 200_000]).unwrap();
    fs::write(
        fixture.source.join("settings.json"),
        b"private-settings-probe",
    )
    .unwrap();
    let database_before = fs::read(&database).unwrap();
    let wal_path = fixture.source.join("legacy.sqlite-wal");
    let wal_before = fs::read(&wal_path).unwrap();
    assert!(!wal_before.is_empty());

    let receipt = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    assert_eq!(database_before, fs::read(&database).unwrap());
    assert_eq!(wal_before, fs::read(&wal_path).unwrap());
    connection
        .batch_execute("INSERT INTO old_history VALUES ('post-upgrade-content');")
        .unwrap();

    let destination = fixture.root.join("restored");
    fixture
        .archive
        .restore_to_new_directory(&receipt, &destination)
        .unwrap();
    let mut restored =
        SqliteConnection::establish(destination.join("legacy.sqlite").to_str().unwrap()).unwrap();
    #[derive(diesel::QueryableByName)]
    struct Row {
        #[diesel(sql_type = diesel::sql_types::Text)]
        content: String,
    }
    let rows = diesel::sql_query("SELECT content FROM old_history")
        .load::<Row>(&mut restored)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "pre-upgrade-private-probe");
    #[derive(diesel::QueryableByName)]
    struct Version {
        #[diesel(sql_type = diesel::sql_types::Integer)]
        user_version: i32,
    }
    assert_eq!(
        diesel::sql_query("PRAGMA user_version")
            .get_result::<Version>(&mut restored)
            .unwrap()
            .user_version,
        73
    );
    assert_eq!(
        fs::read(destination.join("attachments/image.bin")).unwrap(),
        [13; 200_000]
    );
    assert_eq!(
        fs::read(destination.join("settings.json")).unwrap(),
        b"private-settings-probe"
    );
    assert!(destination.join("empty-directory").is_dir());
    let current = diesel::sql_query("SELECT content FROM old_history")
        .load::<Row>(&mut connection)
        .unwrap();
    assert_eq!(current.len(), 2, "还原不能删除当前版本新增资料");
    let archive_bytes = fs::read(
        fixture
            .archive
            .path(uuid::Uuid::from_bytes(receipt.archive_id)),
    )
    .unwrap();
    for probe in [
        b"pre-upgrade-private-probe".as_slice(),
        b"private-settings-probe",
        b"legacy.sqlite",
        b"old-product-1.0",
    ] {
        assert!(!archive_bytes
            .windows(probe.len())
            .any(|window| window == probe));
    }
}

#[test]
fn empty_directory_round_trip_and_reopened_store_verification() {
    let fixture = Fixture::new();
    let receipt = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    let reopened = ProfileBackupArchive::new(fixture.root.join("backups"), fixture.storage.clone());
    reopened.verify(&receipt).unwrap();
    let destination = fixture.root.join("restored");
    reopened
        .restore_to_new_directory(&receipt, &destination)
        .unwrap();
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}

#[test]
fn multiple_archives_never_overwrite_previous_data() {
    let fixture = Fixture::new();
    let path = fixture.source.join("payload");
    fs::write(&path, b"first").unwrap();
    let first = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    fs::write(&path, b"second").unwrap();
    let second = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    assert_ne!(first.archive_id, second.archive_id);
    let destination = fixture.root.join("restored");
    fixture
        .archive
        .restore_to_new_directory(&first, &destination)
        .unwrap();
    assert_eq!(fs::read(destination.join("payload")).unwrap(), b"first");
    fixture.archive.verify(&second).unwrap();
}

#[test]
fn missing_key_is_not_recreated_and_existing_destination_is_never_overwritten() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("payload"), b"unchanged").unwrap();
    let receipt = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    assert!(fixture
        .archive
        .restore_to_new_directory(&receipt, &fixture.source)
        .is_err());
    assert_eq!(
        fs::read(fixture.source.join("payload")).unwrap(),
        b"unchanged"
    );
    fixture
        .storage
        .delete(&key_name(uuid::Uuid::from_bytes(receipt.archive_id)))
        .unwrap();
    assert!(matches!(
        fixture.archive.verify(&receipt),
        Err(ProfileBackupArchiveError::KeyMissing)
    ));
    assert!(fixture.storage.0.lock().unwrap().is_empty());
}

#[test]
fn damaged_archive_and_wrong_version_are_rejected_before_creating_restore_directory() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("payload"), [7; 200_000]).unwrap();
    let receipt = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    let destination = fixture.root.join("restored");
    let mut wrong_version = receipt.clone();
    wrong_version.source.product_version = "not-the-old-version".into();
    assert!(matches!(
        fixture
            .archive
            .restore_to_new_directory(&wrong_version, &destination),
        Err(ProfileBackupArchiveError::StateChanged)
    ));
    assert!(!destination.exists());
    let path = fixture
        .archive
        .path(uuid::Uuid::from_bytes(receipt.archive_id));
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(path, bytes).unwrap();
    let error = fixture
        .archive
        .restore_to_new_directory(&receipt, &destination)
        .unwrap_err();
    assert!(error.source().is_some());
    assert!(!destination.exists());
}

#[test]
fn backup_directory_inside_source_is_rejected_without_modifying_source() {
    let fixture = Fixture::new();
    let nested = fixture.source.join("nested/backups");
    let archive = ProfileBackupArchive::new(nested, fixture.storage.clone());
    assert!(archive.capture(&fixture.source, source_version()).is_err());
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 0);
    assert!(fixture.storage.0.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn symbolic_links_and_special_files_cannot_enter_archives() {
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    let fixture = Fixture::new();
    let external = fixture.root.join("external");
    fs::write(&external, b"not-managed").unwrap();
    let link = fixture.source.join("link");
    symlink(&external, &link).unwrap();
    assert!(fixture
        .archive
        .capture(&fixture.source, source_version())
        .is_err());
    fs::remove_file(link).unwrap();
    let _listener = UnixListener::bind(fixture.source.join("socket")).unwrap();
    assert!(fixture
        .archive
        .capture(&fixture.source, source_version())
        .is_err());
    assert_eq!(fs::read(&external).unwrap(), b"not-managed");
    assert!(fs::read_dir(fixture.root.join("backups"))
        .unwrap()
        .all(|entry| entry.unwrap().path().extension().unwrap() == "partial"));
}

#[test]
fn stream_authenticates_exact_boundaries_and_rejects_truncation_reordering_and_append() {
    let key = MasterKey::from_bytes(&[9; 32]).unwrap();
    for size in [0, 1, 65_535, 65_536, 65_537, 131_072] {
        let plaintext = vec![6; size];
        let mut writer = ArchiveWriter::new(Vec::new(), &key).unwrap();
        for part in plaintext.chunks(317) {
            writer.write_all(part).unwrap();
        }
        let bytes = writer.finish().unwrap();
        let mut recovered = Vec::new();
        ArchiveReader::new(Cursor::new(&bytes), &key)
            .unwrap()
            .read_to_end(&mut recovered)
            .unwrap();
        assert_eq!(plaintext, recovered);
        for length in [0, 8, 27, bytes.len() - 1] {
            assert!(decode(&bytes[..length], &key).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode(&extra, &key).is_err());
        assert!(decode(&bytes, &MasterKey::from_bytes(&[10; 32]).unwrap()).is_err());
        if size == 131_072 {
            let mut reordered = bytes.clone();
            let length = 4 + 65_536 + 16;
            reordered[27..27 + length].copy_from_slice(&bytes[27 + length..27 + 2 * length]);
            assert!(decode(&reordered, &key).is_err());
        }
    }
}

fn decode(bytes: &[u8], key: &MasterKey) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    ArchiveReader::new(Cursor::new(bytes), key)?.read_to_end(&mut output)?;
    Ok(output)
}

#[test]
fn oversized_ciphertext_frame_is_rejected_without_large_allocation() {
    let key = MasterKey::from_bytes(&[9; 32]).unwrap();
    let mut bytes = ArchiveWriter::new(Vec::new(), &key)
        .unwrap()
        .finish()
        .unwrap();
    bytes[27..31].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(decode(&bytes, &key).is_err());
}

#[test]
fn streaming_large_input_uses_bounded_output_frames_and_preserves_write_failure() {
    #[derive(Default)]
    struct BoundedSink {
        largest_write: usize,
        bytes: usize,
    }
    impl Write for BoundedSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.largest_write = self.largest_write.max(bytes.len());
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let key = MasterKey::from_bytes(&[9; 32]).unwrap();
    let mut writer = ArchiveWriter::new(BoundedSink::default(), &key).unwrap();
    let size = 16 * 1024 * 1024;
    io::copy(&mut io::repeat(19).take(size), &mut writer).unwrap();
    let output = writer.finish().unwrap();
    assert!(output.bytes > size as usize);
    assert!(output.largest_write <= 65_536 + 16);

    struct FailingSink;
    impl Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error = match ArchiveWriter::new(FailingSink, &key) {
        Ok(_) => panic!("storage failure must be returned"),
        Err(error) => ProfileBackupArchiveError::from(error),
    };
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::StorageFull
    );
}

#[test]
fn archive_validator_rejects_duplicate_members_links_and_non_directory_parents() {
    use tar::{Builder, EntryType, Header};
    for members in [
        vec![
            ("data", EntryType::Directory),
            ("data/a", EntryType::Regular),
            ("data/a", EntryType::Regular),
        ],
        vec![
            ("data", EntryType::Directory),
            ("data/link", EntryType::Symlink),
        ],
        vec![
            ("data", EntryType::Directory),
            ("data/a", EntryType::Regular),
            ("data/a/b", EntryType::Regular),
        ],
        vec![("outside", EntryType::Directory)],
        vec![
            ("data", EntryType::Directory),
            ("data/a\\b", EntryType::Regular),
        ],
    ] {
        let mut builder = Builder::new(Vec::new());
        let metadata = serde_json::to_vec(&source_version()).unwrap();
        let mut header = Header::new_gnu();
        header.set_size(metadata.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "source.json", metadata.as_slice())
            .unwrap();
        for (path, kind) in members {
            let mut header = Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o600);
            header.set_entry_type(kind);
            if kind.is_symlink() {
                header.set_link_name("outside").unwrap();
            }
            header.set_cksum();
            builder.append_data(&mut header, path, io::empty()).unwrap();
        }
        let bytes = builder.into_inner().unwrap();
        assert!(super::tree::read_tree(Cursor::new(bytes), None).is_err());
    }
}

#[cfg(unix)]
#[test]
fn backup_and_isolated_restore_are_private_to_the_current_user() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::write(fixture.source.join("private"), b"private-data").unwrap();
    let receipt = fixture
        .archive
        .capture(&fixture.source, source_version())
        .unwrap();
    let path = fixture
        .archive
        .path(uuid::Uuid::from_bytes(receipt.archive_id));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let destination = fixture.root.join("restored");
    fixture
        .archive
        .restore_to_new_directory(&receipt, &destination)
        .unwrap();
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(destination.join("private"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn changed_source_between_capture_and_verification_never_publishes_archive() {
    struct MutatingStorage {
        inner: MemoryStorage,
        source: PathBuf,
    }
    impl SecureStoragePort for MutatingStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            let value = self.inner.get(key)?;
            if value.is_some() {
                // 持久密钥回读发生在第一遍目录写出后，精确覆盖第二遍来源校验。
                fs::write(&self.source, b"concurrent-new-content").unwrap();
            }
            Ok(value)
        }
        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.inner.delete(key)
        }
    }
    let fixture = Fixture::new();
    let source = fixture.source.join("payload");
    fs::write(&source, b"original").unwrap();
    let archive = ProfileBackupArchive::new(
        fixture.root.join("backups"),
        Arc::new(MutatingStorage {
            inner: MemoryStorage::default(),
            source: source.clone(),
        }),
    );
    assert!(matches!(
        archive.capture(&fixture.source, source_version()),
        Err(ProfileBackupArchiveError::SourceChanged)
    ));
    assert_eq!(fs::read(source).unwrap(), b"concurrent-new-content");
    assert!(fs::read_dir(fixture.root.join("backups"))
        .unwrap()
        .all(|entry| entry.unwrap().path().extension().unwrap() == "partial"));
}

#[test]
fn secure_storage_failure_retains_source_without_exposing_private_details() {
    struct DeniedStorage;
    impl SecureStoragePort for DeniedStorage {
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Err(SecureStorageError::PermissionDenied(
                "private-diagnostic-probe".into(),
            ))
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), SecureStorageError> {
            panic!("must stop on read failure")
        }
        fn delete(&self, _: &str) -> Result<(), SecureStorageError> {
            panic!("must not remove existing data")
        }
    }
    let fixture = Fixture::new();
    let archive = ProfileBackupArchive::new(fixture.root.join("backups"), Arc::new(DeniedStorage));
    let error = archive
        .capture(&fixture.source, source_version())
        .unwrap_err();
    assert!(matches!(
        error,
        ProfileBackupArchiveError::SecureStorage { .. }
    ));
    assert!(error
        .source()
        .unwrap()
        .downcast_ref::<SecureStorageError>()
        .is_some());
    assert!(!error.to_string().contains("private-diagnostic-probe"));
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 0);
}
