use anyhow::Result;
use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool, PooledConnection};
use diesel::sqlite::SqliteConnection;
use diesel::{connection::SimpleConnection, Connection, RunQueryDsl};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// Embed all diesel migrations at compile time
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

type RawDbPool = Pool<ConnectionManager<SqliteConnection>>;

#[derive(Clone)]
pub struct DbPool {
    inner: Arc<RwLock<RawDbPool>>,
}

impl DbPool {
    pub fn get(
        &self,
    ) -> std::result::Result<
        PooledConnection<ConnectionManager<SqliteConnection>>,
        diesel::r2d2::PoolError,
    > {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get()
    }

    pub fn replace_database(&self, database_url: &str) -> Result<()> {
        let replacement = build_raw_pool(database_url)?;
        run_migrations_raw(&replacement)?;
        install_revision_triggers_raw(&replacement)?;
        *self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = replacement;
        Ok(())
    }

    pub fn detach_to_ephemeral_database(&self) -> Result<()> {
        let manager = ConnectionManager::<SqliteConnection>::new(":memory:");
        let replacement = Pool::builder()
            .max_size(1)
            .connection_customizer(Box::new(SqlitePragmaCustomizer))
            .build(manager)
            .map_err(|error| anyhow::anyhow!("Failed to create ephemeral database: {error}"))?;
        run_migrations_raw(&replacement)?;
        install_revision_triggers_raw(&replacement)?;
        *self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = replacement;
        Ok(())
    }

    pub fn persistent_revision(&self) -> Result<u64> {
        let row =
            diesel::sql_query("SELECT revision FROM uc_database_revision WHERE singleton_id = 1")
                .get_result::<DatabaseRevisionRow>(&mut self.get()?)?;
        u64::try_from(row.revision).map_err(|_| anyhow::anyhow!("database revision is invalid"))
    }
}

#[derive(diesel::QueryableByName)]
struct DatabaseRevisionRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    revision: i64,
}

#[derive(diesel::QueryableByName)]
struct DatabaseTableRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
}

/// Connection customizer that sets per-connection SQLite pragmas on each new connection.
///
/// - `busy_timeout = 5000`: Tells SQLite to wait up to 5 seconds before returning
///   `SQLITE_BUSY`, giving concurrent writers time to finish instead of failing immediately.
/// - `foreign_keys = ON`: Enforces foreign-key constraints for data integrity.
///
/// Note: `journal_mode = WAL` is intentionally NOT set here. WAL mode is a database-file-level
/// setting that persists once set. It is configured once via [`enable_wal_mode`] before
/// pool creation, avoiding "database is locked" errors when r2d2 initializes multiple
/// connections concurrently.
#[derive(Debug)]
struct SqlitePragmaCustomizer;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for SqlitePragmaCustomizer {
    fn on_acquire(
        &self,
        conn: &mut SqliteConnection,
    ) -> std::result::Result<(), diesel::r2d2::Error> {
        use diesel::r2d2::Error::QueryError;

        diesel::sql_query("PRAGMA busy_timeout = 5000")
            .execute(conn)
            .map_err(|e| {
                warn!(error = %e, "Failed to set busy_timeout");
                QueryError(e)
            })?;

        diesel::sql_query("PRAGMA foreign_keys = ON")
            .execute(conn)
            .map_err(|e| {
                warn!(error = %e, "Failed to set foreign_keys=ON");
                QueryError(e)
            })?;

        diesel::sql_query("PRAGMA secure_delete = ON")
            .execute(conn)
            .map_err(|e| {
                warn!(error = %e, "Failed to set secure_delete=ON");
                QueryError(e)
            })?;

        Ok(())
    }
}

/// Enable WAL journal mode on the database using a single dedicated connection.
///
/// WAL (Write-Ahead Logging) is a database-file-level setting that persists once set.
/// By setting it on a single connection before pool creation, we avoid "database is locked"
/// errors that occur when multiple r2d2 pool connections try to set it concurrently during
/// pool initialization.
///
/// # Errors
///
/// Returns an error if the connection cannot be established or the WAL pragma fails.
fn enable_wal_mode(database_url: &str) -> Result<()> {
    let mut conn = SqliteConnection::establish(database_url)
        .map_err(|e| anyhow::anyhow!("Failed to connect for WAL setup: {}", e))?;

    diesel::sql_query("PRAGMA journal_mode = WAL")
        .execute(&mut conn)
        .map_err(|e| anyhow::anyhow!("Failed to set journal_mode=WAL: {}", e))?;

    info!("WAL journal mode enabled");
    Ok(())
}

/// Initialize the database connection pool and apply embedded migrations.
///
/// This must be called once at application startup. On success it returns a ready-to-use
/// `DbPool` with all pending Diesel migrations applied.
///
/// WAL journal mode is set once on a dedicated connection before pool creation to avoid
/// lock contention. Each pool connection automatically gets a 5-second busy timeout and
/// foreign key enforcement via [`SqlitePragmaCustomizer`].
///
/// # Errors
///
/// Returns an `anyhow::Error` if enabling WAL mode, creating the connection pool,
/// obtaining a connection from the pool, or applying migrations fails.
///
/// # Examples
///
/// ```no_run
/// # use uc_infra::db::pool::init_db_pool;
/// let pool = init_db_pool(":memory:").expect("failed to initialize DB pool");
/// // use `pool` to acquire connections: let conn = pool.get().unwrap();
/// ```
pub fn init_db_pool(database_url: &str) -> Result<DbPool> {
    let pool = build_raw_pool(database_url)?;
    run_migrations_raw(&pool)?;
    install_revision_triggers_raw(&pool)?;
    Ok(DbPool {
        inner: Arc::new(RwLock::new(pool)),
    })
}

/// 只为已经完成 migration 的候选 generation 建立 production 连接池。
///
/// 该入口不重跑 migration，也不删除/重建 revision trigger，避免只读回验改变
/// 候选数据库的 schema cookie。调用方必须先通过自己的完整 schema/integrity gate。
pub(crate) fn open_existing_db_pool(database_url: &str) -> Result<DbPool> {
    let manager = ConnectionManager::<SqliteConnection>::new(database_url);
    let pool = Pool::builder()
        .connection_customizer(Box::new(SqlitePragmaCustomizer))
        .build(manager)
        .map_err(|error| anyhow::anyhow!("Failed to open existing database pool: {error}"))?;
    Ok(DbPool {
        inner: Arc::new(RwLock::new(pool)),
    })
}

fn install_revision_triggers_raw(pool: &RawDbPool) -> Result<()> {
    let mut connection = pool.get()?;
    connection.batch_execute(
        "DROP TRIGGER IF EXISTS uc_revision___diesel_schema_migrations_insert;\
         DROP TRIGGER IF EXISTS uc_revision___diesel_schema_migrations_update;\
         DROP TRIGGER IF EXISTS uc_revision___diesel_schema_migrations_delete;",
    )?;
    let tables = diesel::sql_query(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name != '__diesel_schema_migrations' \
         AND name != 'uc_database_revision' ORDER BY name",
    )
    .load::<DatabaseTableRow>(&mut connection)?;
    for table in tables {
        if !table
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(anyhow::anyhow!("database table name is unsupported"));
        }
        for operation in ["insert", "update", "delete"] {
            let sql = format!(
                "CREATE TRIGGER IF NOT EXISTS uc_revision_{table}_{operation} \
                 AFTER {operation} ON \"{table}\" BEGIN \
                 UPDATE uc_database_revision SET revision = revision + 1 WHERE singleton_id = 1; \
                 END",
                table = table.name,
                operation = operation,
            );
            connection.batch_execute(&sql)?;
        }
    }
    Ok(())
}

fn build_raw_pool(database_url: &str) -> Result<RawDbPool> {
    // Set WAL mode once on a single connection before pool creation.
    // WAL is a persistent database-file-level setting, so it only needs to be set once.
    // Doing it here avoids "database is locked" errors when r2d2 initializes multiple
    // connections concurrently, each trying to set WAL in on_acquire.
    enable_wal_mode(database_url)?;

    let manager = ConnectionManager::<SqliteConnection>::new(database_url);

    Pool::builder()
        .connection_customizer(Box::new(SqlitePragmaCustomizer))
        .build(manager)
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))
}

#[cfg(test)]
mod switch_tests {
    use diesel::{Connection, RunQueryDsl, SqliteConnection};
    use diesel_migrations::MigrationHarness;
    use tempfile::tempdir;

    use super::{init_db_pool, MIGRATIONS};

    #[derive(diesel::QueryableByName)]
    struct ValueRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        value: String,
    }

    #[derive(Debug, diesel::QueryableByName)]
    struct TriggerRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        name: String,
    }

    #[derive(diesel::QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    fn seed(path: &std::path::Path, value: &str) {
        let mut connection = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        diesel::sql_query("CREATE TABLE generation_probe (value TEXT NOT NULL)")
            .execute(&mut connection)
            .unwrap();
        diesel::sql_query("INSERT INTO generation_probe (value) VALUES (?)")
            .bind::<diesel::sql_types::Text, _>(value)
            .execute(&mut connection)
            .unwrap();
    }

    #[test]
    fn every_clone_reads_the_replacement_database_after_switch() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let target = directory.path().join("target.sqlite");
        seed(&source, "source");
        seed(&target, "target");
        let pool = init_db_pool(source.to_str().unwrap()).unwrap();
        let repository_pool = pool.clone();
        let before = diesel::sql_query("SELECT value FROM generation_probe")
            .get_result::<ValueRow>(&mut repository_pool.get().unwrap())
            .unwrap();
        assert_eq!(before.value, "source");

        pool.replace_database(target.to_str().unwrap()).unwrap();

        let after = diesel::sql_query("SELECT value FROM generation_probe")
            .get_result::<ValueRow>(&mut repository_pool.get().unwrap())
            .unwrap();
        assert_eq!(after.value, "target");
    }

    #[test]
    fn failed_replacement_leaves_every_clone_on_the_current_database() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let invalid_target = directory.path().join("database-directory");
        seed(&source, "source");
        std::fs::create_dir(&invalid_target).unwrap();
        let pool = init_db_pool(source.to_str().unwrap()).unwrap();
        let repository_pool = pool.clone();

        assert!(pool
            .replace_database(invalid_target.to_str().unwrap())
            .is_err());

        let current = diesel::sql_query("SELECT value FROM generation_probe")
            .get_result::<ValueRow>(&mut repository_pool.get().unwrap())
            .unwrap();
        assert_eq!(current.value, "source");
    }

    #[test]
    fn persistent_revision_tracks_commits_and_survives_reopen() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("revision.sqlite");
        seed(&database, "first");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        let before = pool.persistent_revision().unwrap();
        diesel::sql_query("INSERT INTO generation_probe (value) VALUES ('second')")
            .execute(&mut pool.get().unwrap())
            .unwrap();
        let after = pool.persistent_revision().unwrap();
        assert!(after > before);

        let reopened = init_db_pool(database.to_str().unwrap()).unwrap();
        assert_eq!(reopened.persistent_revision().unwrap(), after);
    }

    #[test]
    fn reverting_revision_migration_removes_runtime_revision_triggers() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("revision-downgrade.sqlite");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        let mut connection = pool.get().unwrap();

        // 先越过准入表与旧准入记录清理，再回滚数据库 revision migration。
        connection.revert_last_migration(MIGRATIONS).unwrap();
        connection.revert_last_migration(MIGRATIONS).unwrap();
        connection.revert_last_migration(MIGRATIONS).unwrap();

        let remaining = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' \
             AND name LIKE 'uc_revision_%' ORDER BY name",
        )
        .load::<TriggerRow>(&mut connection)
        .unwrap()
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
        assert!(remaining.is_empty(), "remaining triggers: {remaining:?}");
    }

    #[test]
    fn retired_legacy_upgrade_records_are_cleared_by_migration() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("retired-legacy-upgrade.sqlite");
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        let mut connection = pool.get().unwrap();

        // 先越过准入表，再回滚到旧准入记录清理 migration 之前。
        connection.revert_last_migration(MIGRATIONS).unwrap();
        connection.revert_last_migration(MIGRATIONS).unwrap();
        diesel::sql_query(
            "INSERT INTO legacy_upgrade_pending_join \
             (peer_lookup_token, encrypted_payload, updated_at_ms) VALUES ('peer', X'01', 1)",
        )
        .execute(&mut connection)
        .unwrap();

        connection.run_pending_migrations(MIGRATIONS).unwrap();

        let row = diesel::sql_query("SELECT COUNT(*) AS count FROM legacy_upgrade_pending_join")
            .get_result::<CountRow>(&mut connection)
            .unwrap();
        assert_eq!(row.count, 0);
    }
}

/// Apply the embedded Diesel migrations using the supplied connection pool.
///
/// Obtains a connection from `pool` and runs all pending embedded migrations compiled into
/// `MIGRATIONS`. Logs progress and returns when migrations complete.
///
/// # Errors
///
/// Returns an error if acquiring a connection from the pool fails or if applying migrations fails.
fn run_migrations_raw(pool: &RawDbPool) -> Result<()> {
    let mut conn = pool.get()?;

    info!("Running database migrations...");
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| anyhow::anyhow!("Migration failed: {}", e))?;
    info!("Database migrations completed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::sql_types::Text;
    use diesel::QueryableByName;

    // Must match the forward-only schema repair migration directory.
    const ENTRY_FILE_SET_REPAIR_VERSION: &str = "20260714000001";

    #[derive(QueryableByName)]
    struct TableColumn {
        #[diesel(sql_type = Text)]
        name: String,
    }

    fn entry_file_set_columns(conn: &mut SqliteConnection) -> Vec<String> {
        diesel::sql_query("PRAGMA table_info(entry_file_set)")
            .load::<TableColumn>(conn)
            .unwrap()
            .into_iter()
            .map(|column| column.name)
            .collect()
    }

    #[test]
    fn pending_migrations_repair_entry_file_set_after_version_collision() {
        let tempdir = tempfile::tempdir().unwrap();
        let db_path = tempdir.path().join("migration-collision.sqlite");
        let database_url = db_path.to_str().unwrap();
        let pool = init_db_pool(database_url).unwrap();

        {
            let mut conn = pool.get().unwrap();
            diesel::sql_query("DROP TABLE entry_file_set")
                .execute(&mut conn)
                .unwrap();
            diesel::sql_query(
                "CREATE TABLE entry_file_set (\
                    entry_id TEXT NOT NULL,\
                    line_index BIGINT NOT NULL,\
                    original_text TEXT NOT NULL,\
                    kind TEXT NOT NULL,\
                    content_hash TEXT,\
                    blob_id TEXT,\
                    size_bytes BIGINT,\
                    exclude_reason TEXT,\
                    root_name_ct BLOB,\
                    PRIMARY KEY (entry_id, line_index),\
                    FOREIGN KEY (entry_id) REFERENCES clipboard_entry(entry_id) ON DELETE CASCADE\
                )",
            )
            .execute(&mut conn)
            .unwrap();
            diesel::sql_query(format!(
                "DELETE FROM __diesel_schema_migrations WHERE version = '{ENTRY_FILE_SET_REPAIR_VERSION}'"
            ))
            .execute(&mut conn)
            .unwrap();
        }
        drop(pool);

        let repaired_pool = init_db_pool(database_url).unwrap();
        let mut conn = repaired_pool.get().unwrap();
        assert_eq!(
            entry_file_set_columns(&mut conn),
            [
                "entry_id",
                "line_index",
                "kind",
                "content_hash",
                "blob_id",
                "size_bytes",
                "exclude_reason",
                "original_text_ct",
                "root_index",
                "relative_path_ct",
                "kind_tag",
                "root_name_ct",
            ]
        );
    }
}
