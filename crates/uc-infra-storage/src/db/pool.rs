use anyhow::{Context, Result};
use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool, PooledConnection};
use diesel::sqlite::SqliteConnection;
use diesel::{connection::SimpleConnection, Connection, RunQueryDsl};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use super::connection::BUSY_TIMEOUT_MS;
use super::ports::DatabaseGenerationMismatch;
use uc_observability_contract::uc_info;

/// Embed all diesel migrations at compile time
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

type RawDbPool = Pool<ConnectionManager<SqliteConnection>>;

/// 暂停时轮询借出连接是否已全部归还的间隔。
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Clone)]
pub struct DbPool {
    inner: Arc<RwLock<PoolSlot>>,
    /// 底层数据库每替换一次加一；读取方据此判断此前读到的资料是否仍属于当前数据库。
    generation: Arc<AtomicU64>,
    /// 暂停时从槽位取下、仍在等待借出连接归还的旧池。期限只结束等待，接续的暂停继续等待同一个池，
    /// 不会因为第二次调用而误报已关闭。
    draining: Arc<Mutex<Option<RawDbPool>>>,
}

/// 当前目标与打开状态；`pool` 为空表示连接已因暂停全部关闭，取连接直接失败而不是重新打开。
struct PoolSlot {
    target: PoolTarget,
    pool: Option<RawDbPool>,
}

/// 内存库没有文件可交还，关闭会丢失其内容，因此暂停时保持打开。
enum PoolTarget {
    File(String),
    Ephemeral,
}

#[derive(Debug, thiserror::Error)]
pub enum DbPoolError {
    #[error("database connections are suspended")]
    Suspended,
    #[error("database pool could not provide a connection")]
    Pool {
        #[from]
        source: diesel::r2d2::PoolError,
    },
}

impl DbPool {
    /// 校验代号和取得池连接共享读锁；替换只能在这两个动作之前或之后发生。
    pub fn get_at_generation(
        &self,
        expected: u64,
    ) -> Result<PooledConnection<ConnectionManager<SqliteConnection>>> {
        let slot = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.generation.load(Ordering::Acquire) != expected {
            return Err(DatabaseGenerationMismatch.into());
        }
        Ok(slot.pool.as_ref().ok_or(DbPoolError::Suspended)?.get()?)
    }

    pub fn get(
        &self,
    ) -> std::result::Result<PooledConnection<ConnectionManager<SqliteConnection>>, DbPoolError>
    {
        let slot = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(slot.pool.as_ref().ok_or(DbPoolError::Suspended)?.get()?)
    }

    pub fn replace_database(&self, database_url: &str) -> Result<()> {
        let replacement = build_raw_pool(database_url)?;
        run_migrations_raw(&replacement)?;
        install_revision_triggers_raw(&replacement)?;
        self.swap(PoolTarget::File(database_url.to_owned()), replacement)
    }

    /// 当前底层数据库的代号；替换数据库后必然改变。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn swap(&self, target: PoolTarget, replacement: RawDbPool) -> Result<()> {
        let mut slot = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 暂停期间所有使用者都已停止；此时替换会在宿主挂起前重新占用文件。
        anyhow::ensure!(slot.pool.is_some(), DbPoolError::Suspended);
        *slot = PoolSlot {
            target,
            pool: Some(replacement),
        };
        // 在持有写锁时推进代号：读到新代号的读取方必然访问新数据库。
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub fn detach_to_ephemeral_database(&self) -> Result<()> {
        let manager = ConnectionManager::<SqliteConnection>::new(":memory:");
        let replacement = Pool::builder()
            .max_size(1)
            .connection_customizer(Box::new(SqlitePragmaCustomizer))
            .build(manager)
            .context("Failed to create ephemeral database")?;
        run_migrations_raw(&replacement)?;
        install_revision_triggers_raw(&replacement)?;
        self.swap(PoolTarget::Ephemeral, replacement)
    }

    /// 关闭文件数据库的全部连接，使本进程不再持有数据库文件及其 SQLite 锁。
    ///
    /// 新的取连接请求立即失败；仍借出的连接不会被强行关闭，本调用一直等到它们归还后才关闭旧池并返回
    /// `true`。`cancelled` 置位时停止等待并返回 `false`，旧池保留给下一次暂停继续等待。
    /// 内存库保持打开。同一实例重复调用不会重复关闭。会阻塞调用线程，不能在异步运行线程上执行。
    pub fn suspend_connections(&self, cancelled: &AtomicBool) -> bool {
        {
            let mut slot = self
                .inner
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if matches!(slot.target, PoolTarget::Ephemeral) {
                return true;
            }
            if let Some(pool) = slot.pool.take() {
                *lock(&self.draining) = Some(pool);
            }
        }
        loop {
            {
                let mut draining = lock(&self.draining);
                let drained = draining.as_ref().is_none_or(|pool| {
                    let state = pool.state();
                    state.connections == state.idle_connections
                });
                if drained {
                    // 最后一个强引用在这里释放：空闲连接随池一起关闭，SQLite 交还文件锁。
                    drop(draining.take());
                    return true;
                }
            }
            if cancelled.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(DRAIN_POLL_INTERVAL);
        }
    }

    /// 重新打开暂停前的同一数据库：不重跑迁移、不改动触发器，也不推进代号。暂停尚未关闭完的旧池直接恢复使用。
    /// 打开后立即取一次连接，数据库无法打开时恢复失败而不是延后到第一次查询。会阻塞调用线程。
    pub fn resume_connections(&self) -> Result<()> {
        let mut slot = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.pool.is_some() {
            return Ok(());
        }
        if let Some(pool) = lock(&self.draining).take() {
            slot.pool = Some(pool);
            return Ok(());
        }
        let PoolTarget::File(database_url) = &slot.target else {
            return Ok(());
        };
        let pool = open_raw_pool(database_url).context("Failed to reopen database pool")?;
        drop(pool.get().context("Failed to reopen database connection")?);
        slot.pool = Some(pool);
        Ok(())
    }

    pub fn persistent_revision(&self) -> Result<u64> {
        let row =
            diesel::sql_query("SELECT revision FROM uc_database_revision WHERE singleton_id = 1")
                .get_result::<DatabaseRevisionRow>(&mut self.get()?)?;
        // discarded-source[int-conversion]: `core::num::TryFromIntError`: the target classification already expresses the range or length mismatch
        u64::try_from(row.revision).map_err(|_| anyhow::anyhow!("database revision is invalid"))
    }

    fn from_raw(target: PoolTarget, pool: RawDbPool) -> Self {
        Self {
            inner: Arc::new(RwLock::new(PoolSlot {
                target,
                pool: Some(pool),
            })),
            generation: Arc::new(AtomicU64::new(0)),
            draining: Arc::new(Mutex::new(None)),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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

        diesel::sql_query(format!("PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}"))
            .execute(conn)
            .map_err(|e| QueryError(e))?;

        diesel::sql_query("PRAGMA foreign_keys = ON")
            .execute(conn)
            .map_err(|e| QueryError(e))?;

        diesel::sql_query("PRAGMA secure_delete = ON")
            .execute(conn)
            .map_err(|e| QueryError(e))?;

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
    let mut conn =
        SqliteConnection::establish(database_url).context("Failed to connect for WAL setup")?;

    diesel::sql_query("PRAGMA journal_mode = WAL")
        .execute(&mut conn)
        .context("Failed to set journal_mode=WAL")?;

    uc_info!("WAL journal mode enabled");
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
/// # use uc_infra_storage::db::pool::init_db_pool;
/// let pool = init_db_pool(":memory:").expect("failed to initialize DB pool");
/// // use `pool` to acquire connections: let conn = pool.get().unwrap();
/// ```
pub fn init_db_pool(database_url: &str) -> Result<DbPool> {
    let pool = build_raw_pool(database_url)?;
    run_migrations_raw(&pool)?;
    install_revision_triggers_raw(&pool)?;
    Ok(DbPool::from_raw(pool_target(database_url), pool))
}

/// 只为已经完成 migration 的候选 generation 建立 production 连接池。
///
/// 该入口不重跑 migration，也不删除/重建 revision trigger，避免只读回验改变
/// 候选数据库的 schema cookie。调用方必须先通过自己的完整 schema/integrity gate。
pub fn open_existing_db_pool(database_url: &str) -> Result<DbPool> {
    let pool = open_raw_pool(database_url).context("Failed to open existing database pool")?;
    Ok(DbPool::from_raw(pool_target(database_url), pool))
}

fn pool_target(database_url: &str) -> PoolTarget {
    if database_url == ":memory:" {
        PoolTarget::Ephemeral
    } else {
        PoolTarget::File(database_url.to_owned())
    }
}

/// 连接只在取用时建立：池不在后台补足空闲连接，暂停取下连接池后不会有工作线程再打开数据库文件。
fn open_raw_pool(database_url: &str) -> Result<RawDbPool> {
    let manager = ConnectionManager::<SqliteConnection>::new(database_url);
    Pool::builder()
        .min_idle(Some(0))
        .connection_customizer(Box::new(SqlitePragmaCustomizer))
        .build(manager)
        .context("Failed to create database pool")
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
    open_raw_pool(database_url)
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

    fn revert_through_migration(connection: &mut SqliteConnection, target_version: &str) {
        while connection
            .applied_migrations()
            .unwrap()
            .iter()
            .any(|version| version.to_string() == target_version)
        {
            connection.revert_last_migration(MIGRATIONS).unwrap();
        }
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
        let generation = repository_pool.generation();

        pool.replace_database(target.to_str().unwrap()).unwrap();

        let after = diesel::sql_query("SELECT value FROM generation_probe")
            .get_result::<ValueRow>(&mut repository_pool.get().unwrap())
            .unwrap();
        assert_eq!(after.value, "target");
        assert_ne!(repository_pool.generation(), generation);
        let replaced = repository_pool.generation();
        pool.detach_to_ephemeral_database().unwrap();
        assert_ne!(repository_pool.generation(), replaced);
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
        let generation = repository_pool.generation();

        assert!(pool
            .replace_database(invalid_target.to_str().unwrap())
            .is_err());
        assert_eq!(repository_pool.generation(), generation);

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

        revert_through_migration(&mut connection, "20260816000002");

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

        revert_through_migration(&mut connection, "20260819000001");
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

    uc_info!("Running database migrations...");
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|error| anyhow::anyhow!(error).context("Migration failed"))?;
    uc_info!("Database migrations completed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{executor::DieselSqliteExecutor, ports::DbExecutor};
    use diesel::sql_types::Text;
    use diesel::QueryableByName;
    use std::sync::mpsc;

    // Must match the forward-only schema repair migration directory.
    const ENTRY_FILE_SET_REPAIR_VERSION: &str = "20260714000001";

    #[test]
    fn connection_acquired_before_replacement_remains_on_old_database() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.sqlite");
        let second = temp.path().join("second.sqlite");
        let pool = init_db_pool(first.to_str().unwrap()).unwrap();
        let executor = DieselSqliteExecutor::new(pool.clone());
        let generation = pool.generation();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            executor.run_at_generation(generation, |conn| {
                acquired_tx.send(())?;
                resume_rx.recv()?;
                diesel::sql_query("PRAGMA user_version = 42").execute(conn)?;
                Ok(())
            })
        });
        acquired_rx.recv().unwrap();
        pool.replace_database(second.to_str().unwrap()).unwrap();
        resume_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        #[derive(QueryableByName)]
        struct Version {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            user_version: i32,
        }
        let version = |pool: &DbPool| {
            diesel::sql_query("PRAGMA user_version")
                .get_result::<Version>(&mut pool.get().unwrap())
                .unwrap()
                .user_version
        };
        assert_eq!(version(&pool), 0);
        let old_pool = init_db_pool(first.to_str().unwrap()).unwrap();
        assert_eq!(version(&old_pool), 42);
        assert!(pool
            .get_at_generation(generation)
            .err()
            .is_some_and(|error| error.is::<DatabaseGenerationMismatch>()));
    }

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
