//! 故障控制连接统一等待业务写事务；触发器产生的真实故障仍立即返回。
use diesel::{Connection, RunQueryDsl, SqliteConnection};
use std::path::Path;

pub(super) fn open_control_connection(database: &Path) -> SqliteConnection {
    let mut connection =
        SqliteConnection::establish(database.to_str().expect("测试数据库路径必须为 UTF-8"))
            .expect("打开 SQLite 故障控制连接");
    // 只等待控制连接的真实写锁竞争，不重试业务操作，也不吞掉注入的存储失败。
    diesel::sql_query("PRAGMA busy_timeout = 5000")
        .execute(&mut connection)
        .expect("设置故障控制连接的写锁等待期限");
    connection
}
