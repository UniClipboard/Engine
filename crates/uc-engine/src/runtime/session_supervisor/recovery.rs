use async_trait::async_trait;
use uc_application::facade::{RebuildNetworkSessionError, RebuildNetworkSessionPort};

use super::SessionSupervisor;
use crate::EngineError;

#[async_trait]
impl RebuildNetworkSessionPort for SessionSupervisor {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        self.rebuild_session().await.map_err(rebuild_error)
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
