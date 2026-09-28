use diesel::connection::SimpleConnection as _;
use diesel::result::Error as DieselError;
use diesel::{Connection as _, SqliteConnection};

pub type DbConn = diesel::SqliteConnection;

/// 运行期连接池与池外独立连接共用的 SQLite 写锁等待上限。
pub(crate) const BUSY_TIMEOUT_MS: u32 = 5_000;

/// 在连接池之外单独打开同一数据库：与池中连接一样先等待其他写事务结束，而不是立即失败。
pub(crate) fn establish_waiting(database: &str) -> anyhow::Result<SqliteConnection> {
    let mut connection = SqliteConnection::establish(database)?;
    connection.batch_execute(&format!("PRAGMA busy_timeout = {BUSY_TIMEOUT_MS};"))?;
    Ok(connection)
}

/// SQLite 报告的锁争用（`SQLITE_BUSY` / `SQLITE_LOCKED`），稍后重试即可继续。
///
/// diesel 把这两个结果码都归入 `DatabaseErrorKind::Unknown`，只能按 SQLite 的固定消息判断；
/// 措辞变化时会退回为非暂时性错误，不会把真正的失败误判为可重试。
pub(crate) fn is_lock_contention(error: &DieselError) -> bool {
    let DieselError::DatabaseError(_, info) = error else {
        return false;
    };
    let message = info.message().to_ascii_lowercase();
    message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("database is busy")
}

#[cfg(test)]
mod tests {
    use diesel::connection::SimpleConnection as _;
    use diesel::Connection as _;

    use super::*;

    #[test]
    fn a_waiting_connection_reports_a_held_write_lock_as_contention() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("contention.sqlite");
        let database = database.to_str().unwrap();
        let mut holder = SqliteConnection::establish(database).unwrap();
        holder
            .batch_execute("CREATE TABLE probe (value INTEGER); BEGIN IMMEDIATE;")
            .unwrap();
        let mut waiting = establish_waiting(database).unwrap();
        waiting.batch_execute("PRAGMA busy_timeout = 50;").unwrap();

        let error = waiting
            .immediate_transaction::<(), DieselError, _>(|_| Ok(()))
            .unwrap_err();

        assert!(is_lock_contention(&error), "{error:?}");
        holder.batch_execute("ROLLBACK;").unwrap();
        waiting
            .immediate_transaction::<(), DieselError, _>(|_| Ok(()))
            .unwrap();
    }
}
