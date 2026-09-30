use std::sync::Arc;

use uc_core::mobile_sync::LanEndpointInfo;
use uc_infra::mobile_sync::InMemoryMobileSyncEndpointInfoAdapter;
use uc_observability_contract::{uc_info, uc_warn};

use crate::{EngineError, MobileLanEndpointUpdate, OperationResult};

pub(crate) struct MobileLanEndpointUpdater {
    endpoint: Arc<InMemoryMobileSyncEndpointInfoAdapter>,
}

impl MobileLanEndpointUpdater {
    pub(crate) fn new(endpoint: Arc<InMemoryMobileSyncEndpointInfoAdapter>) -> Self {
        Self { endpoint }
    }

    pub(crate) async fn update(
        &self,
        update: MobileLanEndpointUpdate,
    ) -> Result<OperationResult, EngineError> {
        // Engine 只记录宿主上报的状态，并未真正绑定；不记 base_url 与 reason。
        match update {
            MobileLanEndpointUpdate::Stopped => {
                uc_info!(state = "stopped", "mobile LAN endpoint state reported");
                self.endpoint.clear().await
            }
            MobileLanEndpointUpdate::Listening { base_url } => {
                uc_info!(state = "listening", "mobile LAN endpoint state reported");
                self.endpoint.set(LanEndpointInfo { url: base_url }).await;
            }
            MobileLanEndpointUpdate::BindFailed { reason } => {
                uc_warn!(
                    state = "bind_failed",
                    error_kind = "mobile_lan_bind_failed",
                    "mobile LAN endpoint state reported"
                );
                self.endpoint.set_bind_failure(reason).await;
            }
        }
        Ok(OperationResult::MobileLanEndpointUpdated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn each_reported_state_is_recorded_without_the_url_or_reason() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let updater = MobileLanEndpointUpdater::new(Arc::new(
            InMemoryMobileSyncEndpointInfoAdapter::default(),
        ));

        for update in [
            MobileLanEndpointUpdate::Listening {
                base_url: "http://192.168.0.7:9000".to_owned(),
            },
            MobileLanEndpointUpdate::BindFailed {
                reason: "PRIVATE_BIND_REASON".to_owned(),
            },
            MobileLanEndpointUpdate::Stopped,
        ] {
            updater.update(update).await.unwrap();
        }

        assert_eq!(logs.count("mobile LAN endpoint state reported"), 3);
        for state in ["listening", "bind_failed", "stopped"] {
            assert!(logs.output().contains(&format!("state=\"{state}\"")));
        }
        assert!(!logs.output().contains("192.168"));
        assert!(!logs.output().contains("PRIVATE"));
    }
}
