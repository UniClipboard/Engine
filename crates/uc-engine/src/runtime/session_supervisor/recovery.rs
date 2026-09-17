use std::sync::atomic::Ordering;

use async_trait::async_trait;
use uc_application::facade::{RebuildNetworkSessionError, RebuildNetworkSessionPort};
use uc_core::FileTransferCancellationReason;
use uc_observability_contract::diagnostics::DiagnosticOperation;

use super::lifecycle::lifecycle_error;
use super::{observe_runtime_operation, SessionSupervisor};
use crate::runtime::operation_unavailable_error;
use crate::EngineError;

#[async_trait]
impl RebuildNetworkSessionPort for SessionSupervisor {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        observe_runtime_operation(DiagnosticOperation::SessionRecovery, async {
            let _lifecycle = self.lifecycle.lock().await;
            if !self.session_recovery_enabled.load(Ordering::Acquire) {
                return Err(operation_unavailable_error());
            }
            self.operations.close_and_wait(None, None).await?;
            self.stop_current_session(FileTransferCancellationReason::ConnectivityRecovery, None)
                .await
                .map_err(lifecycle_error)?;
            self.shutdown_network(None).await?;
            self.install_new_session(false).await
        })
        .await
        .map_err(rebuild_error)
    }
}

fn rebuild_error(source: EngineError) -> RebuildNetworkSessionError {
    let retryable = source.is_retryable();
    RebuildNetworkSessionError::new(source, retryable)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::rebuild_error;
    use crate::{EngineError, EngineErrorCategory};

    #[test]
    fn rebuild_classification_preserves_the_original_failure() {
        for retryable in [true, false] {
            let original = EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, retryable);
            let converted = rebuild_error(original.clone());
            assert_eq!(converted.is_retryable(), retryable);
            let source = converted
                .source()
                .unwrap()
                .downcast_ref::<EngineError>()
                .unwrap();
            assert_eq!(source.code(), original.code());
            assert_eq!(source.category(), original.category());
            assert_eq!(source.is_retryable(), retryable);
        }
    }
}
