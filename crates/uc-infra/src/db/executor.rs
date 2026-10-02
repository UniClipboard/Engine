use diesel::SqliteConnection;
use uc_observability_contract::diagnostics::connectivity::{observe_pairing_wait, LocalWorkStep};

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
        let mut conn =
            observe_pairing_wait(LocalWorkStep::DatabaseConnectionAcquire, || self.pool.get())?;
        f(&mut conn)
    }

    fn database_generation(&self) -> u64 {
        self.pool.generation()
    }
}
