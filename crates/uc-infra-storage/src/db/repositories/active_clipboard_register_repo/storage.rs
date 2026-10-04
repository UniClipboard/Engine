use std::sync::Arc;

use diesel::SqliteConnection;
use tokio::task::spawn_blocking;
use tracing::Span;

use super::{ActiveClipboardRegisterError, DbExecutor, DieselActiveClipboardRegisterRepository};

impl<E: DbExecutor + 'static> DieselActiveClipboardRegisterRepository<E> {
    pub(super) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, ActiveClipboardRegisterError> {
        let executor = Arc::clone(&self.executor);
        let span = Span::current();
        spawn_blocking(move || span.in_scope(|| executor.run(operation)))
            .await
            .map_err(|source| ActiveClipboardRegisterError::Storage(source.into()))?
            .map_err(Into::into)
    }
}
