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

    /// 在指定数据库代号取得连接。可替换数据库的实现必须重写，保证代号检查与取连接原子。
    fn run_at_generation<T>(
        &self,
        generation: u64,
        f: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if generation != 0 || self.database_generation() != 0 {
            return Err(DatabaseGenerationMismatch.into());
        }
        self.run(f)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("database generation changed")]
pub struct DatabaseGenerationMismatch;

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

    fn run_at_generation<U>(
        &self,
        generation: u64,
        f: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<U>,
    ) -> anyhow::Result<U> {
        self.as_ref().run_at_generation(generation, f)
    }
}
