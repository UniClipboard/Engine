use diesel::SqliteConnection;
use std::sync::Arc;

pub trait DbExecutor: Send + Sync {
    fn run<T>(
        &self,
        f: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T>;

    /// 底层数据库的代号，替换数据库后改变；不会替换数据库的执行器保持为 0。
    fn database_generation(&self) -> u64 {
        0
    }
}

// Implement DbExecutor for Arc<T> where T: DbExecutor
// This allows sharing the executor across multiple repositories
impl<T: DbExecutor> DbExecutor for Arc<T> {
    fn run<U>(
        &self,
        f: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<U>,
    ) -> anyhow::Result<U> {
        self.as_ref().run(f)
    }

    fn database_generation(&self) -> u64 {
        T::database_generation(self)
    }
}
