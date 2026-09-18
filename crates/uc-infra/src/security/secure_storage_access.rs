use std::sync::Arc;

use tokio::task::spawn_blocking;
use uc_core::ports::{SecureStorageAccessFailure, SecureStorageError, SecureStoragePort};

/// 异步流程访问同步安全存储的唯一执行入口。
#[derive(Clone)]
pub(crate) struct SecureStorageAccess {
    storage: Arc<dyn SecureStoragePort>,
}

impl SecureStorageAccess {
    pub(crate) fn new(storage: Arc<dyn SecureStoragePort>) -> Self {
        Self { storage }
    }

    pub(crate) async fn execute<T, E, F>(&self, operation: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<SecureStorageError> + Send + 'static,
        F: FnOnce(&dyn SecureStoragePort) -> Result<T, E> + Send + 'static,
    {
        let storage = Arc::clone(&self.storage);
        match spawn_blocking(move || operation(storage.as_ref())).await {
            Ok(result) => result,
            Err(source) => Err(E::from(SecureStorageError::AccessFailed(
                SecureStorageAccessFailure::new(source),
            ))),
        }
    }
}
