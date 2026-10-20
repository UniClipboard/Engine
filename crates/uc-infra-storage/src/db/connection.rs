use std::error::Error;
use std::iter::successors;

use diesel::connection::SimpleConnection as _;
use diesel::result::Error as DieselError;
use diesel::{Connection as _, ConnectionError, SqliteConnection};

pub type DbConn = diesel::SqliteConnection;

/// 运行期连接池与池外独立连接共用的 SQLite 写锁等待上限。
pub(crate) const BUSY_TIMEOUT_MS: u32 = 5_000;

/// 在连接池之外单独打开同一数据库：与池中连接一样先等待其他写事务结束，而不是立即失败。
pub fn establish_waiting(database: &str) -> anyhow::Result<SqliteConnection> {
    let mut connection = SqliteConnection::establish(database)?;
    connection.batch_execute(&format!("PRAGMA busy_timeout = {BUSY_TIMEOUT_MS};"))?;
    Ok(connection)
}

/// SQLite 报告的锁争用（`SQLITE_BUSY` / `SQLITE_LOCKED`），稍后重试即可继续。
///
/// diesel 把这两个结果码都归入 `DatabaseErrorKind::Unknown`，只能按 SQLite 的固定消息判断；
/// 措辞变化时会退回为非暂时性错误，不会把真正的失败误判为可重试。
pub fn is_lock_contention(error: &DieselError) -> bool {
    let DieselError::DatabaseError(_, info) = error else {
        return false;
    };
    let message = info.message().to_ascii_lowercase();
    message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("database is busy")
}

/// 来源链上第一个 SQLite 失败的主结果码分类，只供诊断字段使用，不输出 SQLite 正文。
///
/// diesel 不暴露结果码，只能按 SQLite 为每个主结果码返回的固定消息识别；无法识别时为 `other`。
/// 链上没有 SQLite 失败时返回 `None`。
pub fn sqlite_failure_class(error: &(dyn Error + 'static)) -> Option<&'static str> {
    successors(Some(error), |&current| current.source()).find_map(|current| {
        if let Some(DieselError::DatabaseError(_, info)) = current.downcast_ref::<DieselError>() {
            return Some(classify_sqlite_message(info.message()));
        }
        if let Some(ConnectionError::BadConnection(message)) =
            current.downcast_ref::<ConnectionError>()
        {
            return Some(classify_sqlite_message(message));
        }
        None
    })
}

fn classify_sqlite_message(message: &str) -> &'static str {
    match message.to_ascii_lowercase().as_str() {
        "database is locked" => "busy",
        "database table is locked" => "locked",
        "database disk image is malformed" => "corrupt",
        "file is not a database" => "not_a_database",
        "unable to open database file" => "cannot_open",
        "disk i/o error" => "io_error",
        "database or disk is full" => "full",
        "attempt to write a readonly database" => "read_only",
        _ => "other",
    }
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
