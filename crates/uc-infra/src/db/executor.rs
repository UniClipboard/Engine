use diesel::SqliteConnection;

use crate::db::pool::DbPool;
use crate::db::ports::DbExecutor;

pub struct DieselSqliteExecutor {
    pool: DbPool,
}

impl DieselSqliteExecutor {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

impl DbExecutor for DieselSqliteExecutor {
    fn run<T>(
        &self,
        f: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = self.pool.get()?;
        f(&mut conn)
    }
}
