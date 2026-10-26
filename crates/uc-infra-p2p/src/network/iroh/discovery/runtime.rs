use anyhow::Error;
use std::sync::Arc;

use iroh_mdns_address_lookup::MdnsAddressLookup;
use tokio::task::JoinHandle;
use uc_application::deps::{DiscoveryError, DiscoveryManager};
use uc_core::error_class::ErrorClass;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_contract::uc_warn;

/// 设备节点保留任务所有权；未完整构建或被丢弃时取消，正常关闭时等待实际退出。
pub(in super::super) struct DiscoveryRuntime {
    manager: Arc<DiscoveryManager>,
    handle: Option<JoinHandle<Result<(), DiscoveryError>>>,
    /// 本机 mDNS 不可用时为 `None`，其余发现与关闭流程不变。
    mdns: Option<MdnsAddressLookup>,
}

impl DiscoveryRuntime {
    pub(in super::super) fn start(
        manager: Arc<DiscoveryManager>,
        mdns: Option<MdnsAddressLookup>,
    ) -> Self {
        let running = manager.clone();
        let handle = tokio::spawn(async move {
            let result = running.run().await;
            if let Err(error) = &result {
                uc_warn!(
                    error_class = error.class(),
                    "peer discovery subscription stopped"
                );
            }
            result
        });
        Self {
            manager,
            handle: Some(handle),
            mdns,
        }
    }

    pub(in super::super) async fn shutdown(&mut self) -> Result<(), Error> {
        let mut failure = self.manager.shutdown().err().map(Error::new);
        if let Some(handle) = self.handle.take() {
            handle.abort();
            match handle.await {
                Ok(Err(error)) => {
                    failure.get_or_insert_with(|| error.into());
                }
                Ok(Ok(())) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    record_task_join_failure(DiagnosticTaskKind::PeerDiscovery);
                    failure.get_or_insert_with(|| error.into());
                }
            }
        }
        if let Some(mdns) = &self.mdns {
            if let Err(error) = mdns.shutdown().await {
                record_task_join_failure(DiagnosticTaskKind::PeerDiscovery);
                failure.get_or_insert_with(|| error.into());
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for DiscoveryRuntime {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
        if let Some(mdns) = &self.mdns {
            mdns.abort();
        }
        if self.manager.shutdown().is_err() {
            uc_warn!(
                error_class = "discovery_shutdown_failed",
                "peer discovery directory cleanup failed"
            );
        }
    }
}
