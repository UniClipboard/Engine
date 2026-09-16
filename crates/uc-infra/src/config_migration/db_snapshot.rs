//! Consistent sqlite snapshot for export.
//!
//! The live database is owned by a running process with an open WAL, so a raw
//! file copy of `uniclipboard.db` would be torn. `VACUUM INTO` asks sqlite to
//! write a fully-consistent, fresh database file from the current committed
//! state — it folds in the WAL and produces a single self-contained file with
//! no sidecars. The snapshot is written to a caller-provided scratch path, read
//! back into memory, and the scratch file removed.

use std::path::Path;

use crate::db::pool::DbPool;
use diesel::connection::SimpleConnection;

/// Failures producing a snapshot.
///
/// Every variant keeps the capability failure that caused it as a source, so
/// adapters can classify the real reason (for example `PermissionDenied` or
/// `StorageFull` on the io cause) instead of discarding it. Callers that expose
/// a stable category upward must walk the chain rather than drop it.
#[derive(Debug, thiserror::Error)]
pub enum DbSnapshotError {
    /// No connection could be obtained from the pool.
    #[error("could not acquire a database connection")]
    Connection {
        #[from]
        source: diesel::r2d2::PoolError,
    },
    /// `VACUUM INTO` failed.
    #[error("snapshot query failed")]
    Query {
        #[from]
        source: diesel::result::Error,
    },
    /// The snapshot file could not be read back / cleaned up.
    #[error("snapshot file io failed")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

/// Produce a consistent snapshot of the pool's database and return its bytes.
///
/// `scratch_path` is a writable location the snapshot is materialized at before
/// being read into memory; it must not already exist (`VACUUM INTO` refuses a
/// pre-existing target). The scratch file is removed before returning on the
/// success path.
pub fn snapshot_to_bytes(pool: &DbPool, scratch_path: &Path) -> Result<Vec<u8>, DbSnapshotError> {
    // VACUUM INTO refuses to overwrite; clear any stale scratch file first.
    if scratch_path.exists() {
        std::fs::remove_file(scratch_path)?;
    }
    if let Some(parent) = scratch_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut conn = pool.get()?;

    // Single-quote escaping for the SQL string literal path.
    let path_str = scratch_path.to_string_lossy().replace('\'', "''");
    let sql = format!("VACUUM INTO '{path_str}'");
    conn.batch_execute(&sql)?;
    drop(conn);

    let bytes = std::fs::read(scratch_path)?;
    // Best-effort cleanup; a leftover scratch file is not fatal but we surface
    // a hard failure to read because that would mean an incomplete snapshot.
    let _ = std::fs::remove_file(scratch_path);

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::init_db_pool;
    use diesel::RunQueryDsl;

    fn temp_db_pool() -> (DbPool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("uniclipboard.db");
        let pool = init_db_pool(db_path.to_str().unwrap()).unwrap();
        (pool, dir)
    }

    #[test]
    fn snapshot_produces_a_valid_sqlite_file() {
        let (pool, dir) = temp_db_pool();
        let scratch = dir.path().join("snapshot.db");

        let bytes = snapshot_to_bytes(&pool, &scratch).unwrap();

        // sqlite files begin with the fixed 16-byte header string.
        assert!(bytes.starts_with(b"SQLite format 3\0"));
        // Scratch file was cleaned up.
        assert!(!scratch.exists());
    }

    #[test]
    fn snapshot_is_openable_and_holds_committed_rows() {
        let (pool, dir) = temp_db_pool();

        {
            let mut conn = pool.get().unwrap();
            diesel::sql_query("CREATE TABLE probe (id INTEGER PRIMARY KEY, v TEXT)")
                .execute(&mut conn)
                .unwrap();
            diesel::sql_query("INSERT INTO probe (v) VALUES ('hello')")
                .execute(&mut conn)
                .unwrap();
        }

        let scratch = dir.path().join("snapshot.db");
        let bytes = snapshot_to_bytes(&pool, &scratch).unwrap();

        // Write the snapshot out and open it as a standalone database.
        let restored = dir.path().join("restored.db");
        std::fs::write(&restored, &bytes).unwrap();
        let pool2 = init_db_pool(restored.to_str().unwrap()).unwrap();
        let mut conn2 = pool2.get().unwrap();

        #[derive(diesel::QueryableByName)]
        struct Row {
            #[diesel(sql_type = diesel::sql_types::Text)]
            v: String,
        }
        let rows: Vec<Row> = diesel::sql_query("SELECT v FROM probe")
            .load(&mut conn2)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].v, "hello");
    }

    #[test]
    fn snapshot_overwrites_stale_scratch_file() {
        let (pool, dir) = temp_db_pool();
        let scratch = dir.path().join("snapshot.db");
        std::fs::write(&scratch, b"stale").unwrap();

        let bytes = snapshot_to_bytes(&pool, &scratch).unwrap();
        assert!(bytes.starts_with(b"SQLite format 3\0"));
    }

    #[test]
    fn snapshot_io_failure_keeps_the_io_cause() {
        let (pool, dir) = temp_db_pool();
        // A directory cannot be removed as a regular scratch file, so the first
        // io step fails before any snapshot work happens.
        let scratch = dir.path().join("snapshot.db");
        std::fs::create_dir(&scratch).unwrap();

        let error = snapshot_to_bytes(&pool, &scratch).unwrap_err();

        assert!(matches!(error, DbSnapshotError::Io { .. }));
        let source = std::error::Error::source(&error).expect("io cause is preserved");
        assert!(source.downcast_ref::<std::io::Error>().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_query_failure_keeps_the_database_cause() {
        use std::os::unix::fs::PermissionsExt;

        let (pool, dir) = temp_db_pool();
        let readonly = dir.path().join("readonly");
        std::fs::create_dir(&readonly).unwrap();
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).unwrap();
        let probe = readonly.join("probe");
        if std::fs::write(&probe, b"x").is_ok() {
            // Privileges bypass the read-only bit; the store cannot be made to
            // fail here.
            std::fs::remove_file(&probe).unwrap();
            return;
        }

        let error = snapshot_to_bytes(&pool, &readonly.join("snapshot.db")).unwrap_err();

        assert!(matches!(error, DbSnapshotError::Query { .. }));
        let source = std::error::Error::source(&error).expect("database cause is preserved");
        assert!(source.downcast_ref::<diesel::result::Error>().is_some());
    }
}
