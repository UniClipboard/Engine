use std::error::Error;
use std::fs;
use std::io::{self, Cursor};
use std::path::PathBuf;

use diesel::connection::SimpleConnection;
use diesel::{Connection, RunQueryDsl, SqliteConnection};
use tempfile::{tempdir, TempDir};

use super::{ProfileBackupArchive, ProfileBackupArchiveError, ProfileBackupSource};

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    source: PathBuf,
    archive: ProfileBackupArchive,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        let archive = ProfileBackupArchive::new(root.join("backups"));
        Self {
            _temporary: temporary,
            root,
            source,
            archive,
        }
    }
}

fn source_version() -> ProfileBackupSource {
    ProfileBackupSource {
        product_version: Some("old-product-1.0".into()),
        engine_version: Some("old-engine-0.9".into()),
        platform: "macos".into(),
        architecture: "aarch64".into(),
        installation_channel: Some("direct".into()),
        artifact_digest: Some([42; 32]),
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
        assert!(archive_bytes
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
    let reopened = ProfileBackupArchive::new(fixture.root.join("backups"));
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
fn existing_destination_is_never_overwritten_and_source_deletion_does_not_lose_backup() {
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
    fs::remove_dir_all(&fixture.source).unwrap();
    fixture.archive.verify(&receipt).unwrap();
    let restored = fixture.root.join("restored-after-delete");
    fixture
        .archive
        .restore_to_new_directory(&receipt, &restored)
        .unwrap();
    assert_eq!(fs::read(restored.join("payload")).unwrap(), b"unchanged");
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
    wrong_version.source.product_version = Some("not-the-old-version".into());
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
    let archive = ProfileBackupArchive::new(nested);
    assert!(archive.capture(&fixture.source, source_version()).is_err());
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 0);
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
fn source_recheck_detects_changes_without_a_secure_storage_dependency() {
    let fixture = Fixture::new();
    let source = fixture.source.join("payload");
    fs::write(&source, b"original").unwrap();
    let (_, before) =
        super::tree::write_selected_tree(io::sink(), &fixture.source, &source_version(), &[])
            .unwrap();
    fs::write(&source, b"changed").unwrap();
    let (_, after) =
        super::tree::write_selected_tree(io::sink(), &fixture.source, &source_version(), &[])
            .unwrap();
    assert_ne!(before, after);
}
