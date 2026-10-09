use tokio::task::spawn_blocking;
use tracing::Span;

use super::ProfileContentKeyVaultError;

pub(super) async fn run<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, ProfileContentKeyVaultError> + Send + 'static,
) -> Result<T, ProfileContentKeyVaultError> {
    let span = Span::current();
    spawn_blocking(move || span.in_scope(operation))
        .await
        .map_err(|source| ProfileContentKeyVaultError::Storage {
            source: anyhow::Error::new(source).context("access profile content vault files"),
        })?
}

#[cfg(test)]
mod tests {
    use super::{run, ProfileContentKeyVaultError};
    use std::error::Error as _;
    use tokio::task::JoinError;

    #[tokio::test]
    async fn file_worker_panic_retains_its_source_without_disclosing_it() {
        let result: Result<(), ProfileContentKeyVaultError> =
            run(|| panic!("PRIVATE filesystem failure")).await;
        let error = result.unwrap_err();
        assert!(error.source().is_some());
        assert!(!format!("{error:?}").contains("PRIVATE"));
        let ProfileContentKeyVaultError::Storage { source } = error else {
            panic!("expected storage failure");
        };
        assert!(source.downcast_ref::<JoinError>().unwrap().is_panic());
    }
}
