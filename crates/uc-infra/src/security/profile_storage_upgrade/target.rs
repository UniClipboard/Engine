use std::path::{Path, PathBuf};
use std::sync::Arc;

use diesel::connection::SimpleConnection as _;
use diesel::{Connection as _, RunQueryDsl as _};

use crate::config_migration::db_snapshot;
use crate::db::pool::DbPool;
use crate::security::profile_runtime_layout::{
    control_generation_directory, profile_generation_directory, CONTROL_DATABASE_FILE,
    PAYLOAD_OUTPUT_DIRECTORY, PROFILE_DATABASE_FILE,
};
use crate::security::{AdmissionKeyError, AdmissionKeyManager};
use crate::space::upgrade_registration_to_control_generation;
use uc_core::membership::ActiveSpaceGenerationManifestV2;

use super::journal::UpgradeJournalV1;
use super::ProfileStorageUpgradeError;

pub(super) const PRIMARY_OUTPUT_DIRECTORY: &str = "v3-primary";

/// 升级器内部唯一拥有 source snapshot 与 target generation 物理布局的组件。
pub(super) struct TargetGenerationStager {
    root: PathBuf,
    source_pool: Option<DbPool>,
    keys: Arc<AdmissionKeyManager>,
}

pub(super) struct StagedTarget {
    pub(super) source_snapshot_digest: [u8; 32],
    pub(super) source_database_revision: u64,
}

pub(super) struct SeparatedStores {
    pub(super) profile_database_digest: [u8; 32],
    pub(super) control_database_digest: [u8; 32],
}

const PROFILE_DATA_TABLES: &[&str] = &[
    "active_clipboard_register",
    "blob",
    "blob_reference",
    "clipboard_entry",
    "clipboard_entry_delivery",
    "clipboard_event",
    "clipboard_migration_backup",
    "clipboard_representation_thumbnail",
    "clipboard_selection",
    "clipboard_snapshot_representation",
    "directory_publish_log",
    "entry_file_set",
    "entry_receive_attempt",
    "file_transfer",
    "file_transfer_events",
    "file_transfer_privacy_maintenance",
    "receive_artifact_log",
    "search_document",
    "search_entry_tag",
    "search_index_meta",
    "search_posting",
];

const SPACE_CONTROL_TABLES: &[&str] = &[
    "encrypted_relationship",
    "legacy_space_bootstrap_log",
    "member_revocation_log",
    "membership_ledger_state",
    "mobile_device",
    "relationship_legacy_peer_address",
    "relationship_legacy_space_member",
    "relationship_legacy_trusted_peer",
    "relationship_privacy_maintenance",
    "space_admission_credentials",
    "space_key_epoch_state",
    "ticket_nonce",
];

const PROFILE_COORDINATION_TABLES: &[&str] = &[
    "admission_repository_state",
    "legacy_upgrade_pending_join",
    "workspace_convergence_state",
    "workspace_convergence_v3_active",
    "workspace_convergence_v3_migrations",
    "workspace_convergence_v3_slots",
];

const TECHNICAL_TABLES: &[&str] = &["__diesel_schema_migrations", "uc_database_revision"];
const OPTIONAL_RETIRED_TABLES: &[&str] = &[
    "ticket_nonce",
    "relationship_legacy_peer_address",
    "relationship_legacy_space_member",
    "relationship_legacy_trusted_peer",
];

impl TargetGenerationStager {
    pub(super) fn new(root: PathBuf, source_pool: DbPool, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            root,
            source_pool: Some(source_pool),
            keys,
        }
    }

    pub(super) fn cleanup_only(root: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            root,
            source_pool: None,
            keys,
        }
    }

    pub(super) fn stage(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<StagedTarget, ProfileStorageUpgradeError> {
        let paths = self.paths(journal);
        // Detected 的候选从未激活；重启或旧版写入后不能复用过期转换输出。
        for directory in [
            profile_generation_directory(&self.root, journal.target_profile_data_generation()),
            control_generation_directory(&self.root, journal.target_space_control_generation()),
        ] {
            match std::fs::remove_dir_all(directory) {
                Ok(()) => {}
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(storage_error(source)),
            }
        }
        let source_database_revision = self.source_revision()?;
        let source_pool =
            self.source_pool
                .as_ref()
                .ok_or_else(|| ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!(
                        "profile upgrade source was requested from cleanup-only recovery"
                    ),
                })?;
        let snapshot =
            db_snapshot::snapshot_to_bytes(source_pool, &paths.scratch).map_err(|source| {
                ProfileStorageUpgradeError::Storage {
                    source: anyhow::Error::new(source)
                        .context("snapshot source profile database for V3 staging"),
                }
            })?;
        if self.source_revision()? != source_database_revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        let source_snapshot_digest = *blake3::hash(&snapshot).as_bytes();
        write_target(&paths.profile_database, &snapshot)?;
        write_target(&paths.control_database, &snapshot)?;
        self.verify_with_digest(journal, source_snapshot_digest)?;
        Ok(StagedTarget {
            source_snapshot_digest,
            source_database_revision,
        })
    }

    pub(super) fn verify(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let digest = journal.source_snapshot_digest().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade snapshot digest is missing"),
            }
        })?;
        let revision = journal.source_database_revision().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade source revision is missing"),
            }
        })?;
        if self.source_revision()? != revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        self.verify_with_digest(journal, digest)
    }

    pub(super) fn separate(
        &self,
        journal: &UpgradeJournalV1,
        source: Option<&ActiveSpaceGenerationManifestV2>,
    ) -> Result<SeparatedStores, ProfileStorageUpgradeError> {
        if !journal.matches_source(source) {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        self.verify(journal)?;
        let paths = self.paths(journal);
        separate_database(&paths.profile_database, SPACE_CONTROL_TABLES)?;
        let mut control_excluded =
            Vec::with_capacity(PROFILE_DATA_TABLES.len() + PROFILE_COORDINATION_TABLES.len());
        control_excluded.extend_from_slice(PROFILE_DATA_TABLES);
        control_excluded.extend_from_slice(PROFILE_COORDINATION_TABLES);
        separate_database(&paths.control_database, &control_excluded)?;
        match source {
            Some(source) => upgrade_registration_to_control_generation(
                &paths.control_database,
                &self.keys,
                source,
                *journal.target_space_control_generation(),
            )
            .map_err(map_credential_upgrade_error)?,
            None => ensure_tables_empty(&paths.control_database, &["space_admission_credentials"])?,
        }
        let profile_database_digest = file_digest(&paths.profile_database)?;
        let control_database_digest = file_digest(&paths.control_database)?;
        let source_revision = journal.source_database_revision().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade source revision is missing"),
            }
        })?;
        if self.source_revision()? != source_revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        Ok(SeparatedStores {
            profile_database_digest,
            control_database_digest,
        })
    }

    pub(super) fn verify_separated(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let paths = self.paths(journal);
        let expected_profile = journal.profile_database_digest().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile data target digest is missing"),
            }
        })?;
        let expected_control = journal.control_database_digest().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("space control target digest is missing"),
            }
        })?;
        if file_digest(&paths.profile_database)? != expected_profile
            || file_digest(&paths.control_database)? != expected_control
        {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade separated store digest mismatch"),
            });
        }
        let revision = journal.source_database_revision().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade source revision is missing"),
            }
        })?;
        if self.source_revision()? != revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        Ok(())
    }

    pub(super) fn verify_source_revision(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let revision = journal.source_database_revision().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade source revision is missing"),
            }
        })?;
        if self.source_revision()? != revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        Ok(())
    }

    pub(super) fn source_changed(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<bool, ProfileStorageUpgradeError> {
        match journal.source_database_revision() {
            Some(revision) => Ok(self.source_revision()? != revision),
            None => Ok(false),
        }
    }

    pub(super) fn staged_snapshot_matches(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<bool, ProfileStorageUpgradeError> {
        let expected = journal.source_snapshot_digest().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade snapshot digest is missing"),
            }
        })?;
        let paths = self.paths(journal);
        for path in [&paths.profile_database, &paths.control_database] {
            match std::fs::read(path) {
                Ok(bytes) if *blake3::hash(&bytes).as_bytes() == expected => {}
                Ok(_) => return Ok(false),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(source) => return Err(storage_error(source)),
            }
        }
        Ok(true)
    }

    /// 按最终双库布局验证所有业务 row 仍由唯一 store 拥有。
    pub(super) fn verify_runtime_row_ownership(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let paths = self.paths(journal);
        ensure_tables_empty(
            &paths.payload_output.join(PROFILE_DATABASE_FILE),
            SPACE_CONTROL_TABLES,
        )?;
        let mut control_forbidden =
            Vec::with_capacity(PROFILE_DATA_TABLES.len() + PROFILE_COORDINATION_TABLES.len());
        control_forbidden.extend_from_slice(PROFILE_DATA_TABLES);
        control_forbidden.extend_from_slice(PROFILE_COORDINATION_TABLES);
        ensure_tables_empty(&paths.control_database, &control_forbidden)
    }

    fn source_revision(&self) -> Result<u64, ProfileStorageUpgradeError> {
        let source_pool =
            self.source_pool
                .as_ref()
                .ok_or_else(|| ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!(
                        "profile upgrade source was requested from cleanup-only recovery"
                    ),
                })?;
        source_pool
            .persistent_revision()
            .map_err(|source| ProfileStorageUpgradeError::Storage {
                source: source.context("read source profile database revision for V3 staging"),
            })
    }

    fn verify_with_digest(
        &self,
        journal: &UpgradeJournalV1,
        expected: [u8; 32],
    ) -> Result<(), ProfileStorageUpgradeError> {
        let paths = self.paths(journal);
        for path in [&paths.profile_database, &paths.control_database] {
            let bytes = std::fs::read(path).map_err(storage_error)?;
            if *blake3::hash(&bytes).as_bytes() != expected {
                return Err(ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!("profile upgrade staged database digest mismatch"),
                });
            }
        }
        Ok(())
    }

    pub(super) fn paths(&self, journal: &UpgradeJournalV1) -> TargetPaths {
        let profile_directory =
            profile_generation_directory(&self.root, journal.target_profile_data_generation());
        let control_directory =
            control_generation_directory(&self.root, journal.target_space_control_generation());
        TargetPaths {
            scratch: self
                .root
                .join("profile-storage-upgrade")
                .join("source.snapshot.tmp"),
            profile_database: profile_directory.join(PROFILE_DATABASE_FILE),
            primary_output: profile_directory.join(PRIMARY_OUTPUT_DIRECTORY),
            payload_output: profile_directory.join(PAYLOAD_OUTPUT_DIRECTORY),
            control_database: control_directory.join(CONTROL_DATABASE_FILE),
        }
    }
}

pub(super) fn legacy_space_generation_directory(
    generation_root: &Path,
    space_id: &str,
    generation: &[u8; 16],
) -> PathBuf {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-generation-directory/v1\0");
    hasher.update(space_id.as_bytes());
    hasher.update(generation);
    let digest: [u8; 32] = hasher.finalize().into();
    generation_root.join(legacy_generation_directory_name(&digest))
}

fn legacy_generation_directory_name(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(32);
    for byte in &digest[..16] {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

pub(super) struct TargetPaths {
    pub(super) scratch: PathBuf,
    pub(super) profile_database: PathBuf,
    pub(super) primary_output: PathBuf,
    pub(super) payload_output: PathBuf,
    pub(super) control_database: PathBuf,
}

#[derive(diesel::QueryableByName)]
struct TableNameRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
}

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

fn ensure_tables_empty(path: &Path, tables: &[&str]) -> Result<(), ProfileStorageUpgradeError> {
    let database = path
        .to_str()
        .ok_or_else(|| ProfileStorageUpgradeError::Corrupt {
            source: anyhow::anyhow!("runtime generation path is invalid"),
        })?;
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database).map_err(|source| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("open runtime generation for ownership validation"),
            }
        })?;
    let actual = load_table_names(&mut connection)?;
    for table in tables {
        if !actual.contains(*table) {
            if OPTIONAL_RETIRED_TABLES.contains(table) {
                continue;
            }
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("runtime generation is missing an owned table"),
            });
        }
        let row = diesel::sql_query(format!("SELECT COUNT(*) AS count FROM \"{table}\""))
            .get_result::<CountRow>(&mut connection)
            .map_err(|source| ProfileStorageUpgradeError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("validate runtime generation row ownership"),
            })?;
        if row.count != 0 {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!(
                    "runtime generation contains rows owned by the other store"
                ),
            });
        }
    }
    Ok(())
}

fn separate_database(
    path: &Path,
    excluded_tables: &[&str],
) -> Result<(), ProfileStorageUpgradeError> {
    let database = path
        .to_str()
        .ok_or_else(|| ProfileStorageUpgradeError::Storage {
            source: anyhow::anyhow!("profile upgrade target path is invalid"),
        })?;
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database).map_err(|source| {
            ProfileStorageUpgradeError::Storage {
                source: anyhow::Error::new(source).context("open profile upgrade target database"),
            }
        })?;
    connection
        .batch_execute("PRAGMA journal_mode = DELETE; PRAGMA foreign_keys = OFF;")
        .map_err(database_error)?;
    let actual = validate_table_ownership(&mut connection)?;
    connection
        .transaction::<_, diesel::result::Error, _>(|connection| {
            for table in excluded_tables {
                if !actual.contains(*table) {
                    continue;
                }
                diesel::sql_query(format!("DELETE FROM \"{table}\"")).execute(connection)?;
            }
            Ok(())
        })
        .map_err(database_error)?;
    connection
        .batch_execute("VACUUM; PRAGMA foreign_keys = ON;")
        .map_err(database_error)?;
    drop(connection);
    crate::fs::durability::sync_existing_file(path).map_err(storage_error)?;
    Ok(())
}

fn validate_table_ownership(
    connection: &mut diesel::sqlite::SqliteConnection,
) -> Result<std::collections::BTreeSet<String>, ProfileStorageUpgradeError> {
    let mut declared = std::collections::BTreeSet::new();
    for table in PROFILE_DATA_TABLES
        .iter()
        .chain(SPACE_CONTROL_TABLES)
        .chain(PROFILE_COORDINATION_TABLES)
        .chain(TECHNICAL_TABLES)
    {
        if !declared.insert(*table) {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade table ownership is duplicated"),
            });
        }
    }
    let actual = load_table_names(connection)?;
    let unknown = actual
        .iter()
        .filter(|name| !declared.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let missing = declared
        .iter()
        .filter(|name| !actual.contains(**name) && !OPTIONAL_RETIRED_TABLES.contains(name))
        .copied()
        .collect::<Vec<_>>();
    if !unknown.is_empty() || !missing.is_empty() {
        return Err(ProfileStorageUpgradeError::Corrupt {
            source: anyhow::anyhow!(
                "profile upgrade table ownership is incomplete; unknown={unknown:?}; missing={missing:?}"
            ),
        });
    }
    Ok(actual)
}

fn load_table_names(
    connection: &mut diesel::sqlite::SqliteConnection,
) -> Result<std::collections::BTreeSet<String>, ProfileStorageUpgradeError> {
    diesel::sql_query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .load::<TableNameRow>(connection)
        .map(|rows| {
            rows.into_iter()
                .map(|row| row.name)
                .filter(|name| !name.starts_with("sqlite_"))
                .collect()
        })
        .map_err(database_error)
}

pub(super) fn file_digest(path: &Path) -> Result<[u8; 32], ProfileStorageUpgradeError> {
    let bytes = std::fs::read(path).map_err(storage_error)?;
    Ok(*blake3::hash(&bytes).as_bytes())
}

fn database_error(source: diesel::result::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Storage {
        source: anyhow::Error::new(source).context("separate profile upgrade target stores"),
    }
}

fn map_credential_upgrade_error(source: anyhow::Error) -> ProfileStorageUpgradeError {
    if source.downcast_ref::<AdmissionKeyError>().is_some() {
        ProfileStorageUpgradeError::Security {
            source: source.context("convert admission credential control-generation scope"),
        }
    } else if source.downcast_ref::<diesel::result::Error>().is_some()
        || source.downcast_ref::<diesel::ConnectionError>().is_some()
    {
        ProfileStorageUpgradeError::Storage {
            source: source.context("convert admission credential control-generation scope"),
        }
    } else {
        ProfileStorageUpgradeError::Corrupt {
            source: source.context("validate admission credential control-generation scope"),
        }
    }
}

fn write_target(path: &Path, bytes: &[u8]) -> Result<(), ProfileStorageUpgradeError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProfileStorageUpgradeError::Storage {
            source: anyhow::anyhow!("profile upgrade target parent is missing"),
        })?;
    std::fs::create_dir_all(parent).map_err(storage_error)?;
    if path.is_file() {
        let existing = std::fs::read(path).map_err(storage_error)?;
        if existing == bytes {
            return Ok(());
        }
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file_atomically(&temporary, path)?;
        sync_parent_directory(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(storage_error)
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide = |path: &Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn storage_error(source: std::io::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Storage {
        source: anyhow::Error::new(source).context("persist profile upgrade target generation"),
    }
}

#[cfg(test)]
mod tests {
    use diesel::connection::SimpleConnection as _;
    use diesel::RunQueryDsl as _;

    use super::{ensure_tables_empty, separate_database, SPACE_CONTROL_TABLES};
    use crate::db::pool::init_db_pool;

    #[test]
    fn final_profile_ownership_rejects_space_control_rows() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("profile.sqlite");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        diesel::sql_query(
            "INSERT INTO membership_ledger_state (singleton_id, encrypted_payload) \
             VALUES (1, X'010203')",
        )
        .execute(&mut pool.get().unwrap())
        .unwrap();

        assert!(ensure_tables_empty(&database, SPACE_CONTROL_TABLES).is_err());
    }

    #[test]
    fn separation_accepts_retired_relationship_tables_already_removed() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("alpha4.sqlite");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        pool.get()
            .unwrap()
            .batch_execute(
                "DROP TABLE relationship_legacy_space_member;\
                 DROP TABLE relationship_legacy_trusted_peer;\
                 DROP TABLE relationship_legacy_peer_address;",
            )
            .unwrap();
        drop(pool);

        separate_database(&database, SPACE_CONTROL_TABLES)
            .expect("already-cleaned retired relationship tables are a valid source");
    }

    #[test]
    fn separation_still_rejects_a_missing_required_table() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("missing-required.sqlite");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        pool.get()
            .unwrap()
            .batch_execute("DROP TABLE clipboard_entry;")
            .unwrap();
        drop(pool);

        assert!(separate_database(&database, SPACE_CONTROL_TABLES).is_err());
    }

    #[test]
    fn retired_ticket_nonces_are_owned_by_the_control_store() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let profile = directory.path().join("profile.sqlite");
        let control = directory.path().join("control.sqlite");
        let pool = init_db_pool(source.to_str().unwrap()).unwrap();
        pool.get().unwrap().batch_execute(
            "CREATE TABLE ticket_nonce (nonce_hash BLOB PRIMARY KEY NOT NULL, expires_at INTEGER NOT NULL, created_at INTEGER NOT NULL, redeemed_at INTEGER);\
             CREATE INDEX idx_ticket_nonce_expires_at ON ticket_nonce (expires_at);\
             INSERT INTO ticket_nonce VALUES (X'010203', 100, 1, NULL);\
             PRAGMA wal_checkpoint(TRUNCATE);",
        ).unwrap();
        drop(pool);
        std::fs::copy(&source, &profile).unwrap();
        std::fs::copy(&source, &control).unwrap();
        separate_database(&profile, SPACE_CONTROL_TABLES).unwrap();
        separate_database(&control, super::PROFILE_DATA_TABLES).unwrap();
        use diesel::Connection as _;
        for (database, expected) in [(&source, 1), (&profile, 0), (&control, 1)] {
            let mut connection =
                diesel::sqlite::SqliteConnection::establish(database.to_str().unwrap()).unwrap();
            let row = diesel::sql_query("SELECT COUNT(*) AS count FROM ticket_nonce")
                .get_result::<super::CountRow>(&mut connection)
                .unwrap();
            assert_eq!(row.count, expected);
        }
    }

    #[test]
    #[ignore = "requires an isolated database copy in UC_LEGACY_DATABASE_COPY"]
    fn supplied_legacy_database_separates_without_changing_the_source() {
        let source = std::path::PathBuf::from(std::env::var_os("UC_LEGACY_DATABASE_COPY").unwrap());
        let before = std::fs::read(&source).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile.sqlite");
        let control = directory.path().join("control.sqlite");
        std::fs::copy(&source, &profile).unwrap();
        std::fs::copy(&source, &control).unwrap();
        separate_database(&profile, SPACE_CONTROL_TABLES).unwrap();
        let excluded: Vec<&str> = super::PROFILE_DATA_TABLES
            .iter()
            .chain(super::PROFILE_COORDINATION_TABLES)
            .copied()
            .collect();
        separate_database(&control, &excluded).unwrap();
        ensure_tables_empty(&profile, SPACE_CONTROL_TABLES).unwrap();
        ensure_tables_empty(&control, &excluded).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), before);
    }
}
