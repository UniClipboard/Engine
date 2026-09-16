use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write as _;
use std::path::Path;

use async_trait::async_trait;
use diesel::connection::SimpleConnection as _;
use diesel::{Connection as _, RunQueryDsl as _, SqliteConnection};
use uc_core::ports::space::{DeriveSpaceSubkeyPort, SpaceAccessError};

use super::{inconsistent, storage, SpaceControlGenerationError};
use crate::db::pool::{init_db_pool, open_existing_db_pool, DbPool};
use crate::space::InMemorySession;

pub(super) struct TargetSessionSubkeyDeriver(InMemorySession);

impl TargetSessionSubkeyDeriver {
    pub(super) fn new(session: InMemorySession) -> Self {
        Self(session)
    }
}

#[async_trait]
impl DeriveSpaceSubkeyPort for TargetSessionSubkeyDeriver {
    async fn derive_subkey(&self, salt: &[u8], info: &[u8]) -> Result<[u8; 32], SpaceAccessError> {
        self.0
            .derive_stable_subkey(salt, info)
            .map_err(|source| SpaceAccessError::Internal(source.to_string()))
    }
}

pub(super) struct ControlGenerationLease {
    _file: File,
}

pub(super) fn acquire_lease(
    generation_parent: &Path,
) -> Result<ControlGenerationLease, SpaceControlGenerationError> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(generation_parent.join(".space-control-generation.lease"))
        .map_err(|source| storage(anyhow::Error::new(source)))?;
    match crate::fs::file_lock::try_lock_exclusive(&file) {
        Ok(()) => Ok(ControlGenerationLease { _file: file }),
        Err(TryLockError::WouldBlock) => Err(SpaceControlGenerationError::Busy {
            source: anyhow::anyhow!("space control generation lease is held"),
        }),
        Err(TryLockError::Error(source)) => Err(storage(anyhow::Error::new(source))),
    }
}

pub(super) fn open_pool(database: &Path) -> Result<DbPool, SpaceControlGenerationError> {
    let database = database
        .to_str()
        .ok_or_else(|| storage(anyhow::anyhow!("control database path is invalid")))?;
    init_db_pool(database).map_err(storage)
}

pub(super) fn open_existing_pool(database: &Path) -> Result<DbPool, SpaceControlGenerationError> {
    let database = database
        .to_str()
        .ok_or_else(|| storage(anyhow::anyhow!("control database path is invalid")))?;
    open_existing_db_pool(database).map_err(storage)
}

pub(super) fn compact_database(database: &Path) -> Result<(), SpaceControlGenerationError> {
    let database_value = database
        .to_str()
        .ok_or_else(|| storage(anyhow::anyhow!("control database path is invalid")))?;
    let mut connection = SqliteConnection::establish(database_value).map_err(|source| {
        storage(anyhow::Error::new(source).context("open prepared control database"))
    })?;
    connection
        .batch_execute("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE; VACUUM;")
        .map_err(|source| {
            storage(anyhow::Error::new(source).context("compact prepared control database"))
        })?;
    drop(connection);
    crate::fs::durability::sync_existing_file(database)
        .map_err(|source| storage(anyhow::Error::new(source)))?;
    let parent = database
        .parent()
        .ok_or_else(|| storage(anyhow::anyhow!("control database parent is missing")))?;
    sync_directory(parent)
}

/// 对仍由运行期 pool 持有的 mutable target 排空 WAL，不切换 journal mode，
/// 也不执行会与池中连接竞争的 VACUUM。
pub(super) fn checkpoint_database(
    pool: &DbPool,
    database: &Path,
) -> Result<(), SpaceControlGenerationError> {
    let mut connection = pool
        .get()
        .map_err(|source| storage(anyhow::Error::new(source)))?;
    let checkpoint = diesel::sql_query("PRAGMA wal_checkpoint(TRUNCATE)")
        .get_result::<CheckpointRow>(&mut connection)
        .map_err(|source| {
            storage(anyhow::Error::new(source).context("checkpoint mutable control database"))
        })?;
    drop(connection);
    if checkpoint.busy != 0 || (checkpoint.log >= 0 && checkpoint.checkpointed < checkpoint.log) {
        return Err(SpaceControlGenerationError::Busy {
            source: anyhow::anyhow!("mutable control database checkpoint is incomplete"),
        });
    }
    crate::fs::durability::sync_existing_file(database)
        .map_err(|source| storage(anyhow::Error::new(source)))?;
    let parent = database
        .parent()
        .ok_or_else(|| storage(anyhow::anyhow!("control database parent is missing")))?;
    sync_directory(parent)
}

#[derive(diesel::QueryableByName)]
struct CheckpointRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    busy: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    log: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    checkpointed: i64,
}

#[derive(diesel::QueryableByName)]
struct IntegrityRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    integrity_check: String,
}

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

pub(super) fn verify_sqlite(database: &Path) -> Result<(), SpaceControlGenerationError> {
    let database = database
        .to_str()
        .ok_or_else(|| inconsistent(anyhow::anyhow!("control database path is invalid")))?;
    let mut connection = SqliteConnection::establish(database).map_err(|source| {
        inconsistent(anyhow::Error::new(source).context("open control database for validation"))
    })?;
    let integrity = diesel::sql_query("PRAGMA integrity_check")
        .get_result::<IntegrityRow>(&mut connection)
        .map_err(|source| {
            inconsistent(anyhow::Error::new(source).context("validate control database integrity"))
        })?;
    let foreign_keys = diesel::sql_query("SELECT COUNT(*) AS count FROM pragma_foreign_key_check")
        .get_result::<CountRow>(&mut connection)
        .map_err(|source| {
            inconsistent(anyhow::Error::new(source).context("validate control database references"))
        })?;
    if integrity.integrity_check != "ok" || foreign_keys.count != 0 {
        return Err(inconsistent(anyhow::anyhow!(
            "control database validation failed"
        )));
    }
    Ok(())
}

pub(super) fn database_digest(database: &Path) -> Result<[u8; 32], SpaceControlGenerationError> {
    let bytes = std::fs::read(database).map_err(|source| storage(anyhow::Error::new(source)))?;
    Ok(*blake3::hash(&bytes).as_bytes())
}

pub(super) fn write_new_database(
    database: &Path,
    bytes: &[u8],
) -> Result<(), SpaceControlGenerationError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(database)
        .map_err(|source| storage(anyhow::Error::new(source)))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| storage(anyhow::Error::new(source)))
}

pub(super) fn sync_directory(directory: &Path) -> Result<(), SpaceControlGenerationError> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|source| storage(anyhow::Error::new(source)))?;
    }
    Ok(())
}

pub(super) fn remove_directory_if_present(
    directory: &Path,
) -> Result<(), SpaceControlGenerationError> {
    match std::fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(storage(anyhow::Error::new(source))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_rejects_an_active_reader_and_succeeds_after_release() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("control.sqlite");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        pool.get().unwrap().batch_execute("CREATE TABLE checkpoint_probe (value INTEGER); INSERT INTO checkpoint_probe VALUES (1);").unwrap();
        let mut reader = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
        reader
            .batch_execute("BEGIN; SELECT * FROM checkpoint_probe;")
            .unwrap();
        pool.get()
            .unwrap()
            .batch_execute("INSERT INTO checkpoint_probe VALUES (2);")
            .unwrap();
        let result = checkpoint_database(&pool, &database);
        reader.batch_execute("ROLLBACK;").unwrap();
        assert!(
            matches!(result, Err(SpaceControlGenerationError::Busy { .. })),
            "checkpoint must not report an incomplete WAL checkpoint as success"
        );
        checkpoint_database(&pool, &database).unwrap();
    }
}
