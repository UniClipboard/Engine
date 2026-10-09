use std::future::Future;
use std::sync::Arc;

use tracing::Instrument;
use uc_observability_contract::diagnostics::ObservationContext;

use super::{ProfileContentKeyVault, ProfileContentKeyVaultError};

impl ProfileContentKeyVault {
    pub(super) async fn run_owned<T, F>(
        &self,
        operation: impl FnOnce(Self) -> F + Send + 'static,
    ) -> Result<T, ProfileContentKeyVaultError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, ProfileContentKeyVaultError>> + Send + 'static,
    {
        let owner = Self {
            persistence: Arc::clone(&self.persistence),
            io_lock: Arc::clone(&self.io_lock),
            reads: Arc::clone(&self.reads),
        };
        let observation = ObservationContext::capture();
        // 所有者持有实际访问和租约，等待方取消只丢弃结果接收。
        tokio::spawn(observation.scope(async move { operation(owner).await }.in_current_span()))
            .await
            .map_err(|source| ProfileContentKeyVaultError::Storage {
                source: anyhow::Error::new(source).context("complete profile content vault access"),
            })?
    }
}
