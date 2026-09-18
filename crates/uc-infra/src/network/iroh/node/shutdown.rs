use std::time::Duration;
use std::{error::Error, fmt};

use tokio::time::{timeout_at, Instant};
use tracing::{debug, instrument, warn};

use super::IrohNode;

const ROUTER_WATCHDOG: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
#[error("network router stopped before shutdown was requested")]
struct RouterStoppedBeforeShutdown;

pub struct IrohNodeShutdownError {
    failures: Vec<anyhow::Error>,
}

impl IrohNodeShutdownError {
    pub fn failures(&self) -> &[anyhow::Error] {
        &self.failures
    }
}

impl fmt::Debug for IrohNodeShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IrohNodeShutdownError")
            .field("failure_count", &self.failures.len())
            .finish()
    }
}

impl fmt::Display for IrohNodeShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("network node did not stop cleanly")
    }
}

impl Error for IrohNodeShutdownError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.failures.first().map(|error| error.as_ref())
    }
}

impl IrohNode {
    /// 等待节点与协议处理器实际关闭；watchdog 只诊断慢收尾，不丢弃持有资源的 future。
    pub async fn shutdown(self) -> Result<(), IrohNodeShutdownError> {
        self.shutdown_at(None).await
    }

    /// 有宿主期限时，内部宽限和慢收尾诊断共用该期限；超时后仍等待实际析构。
    #[instrument(skip_all)]
    pub async fn shutdown_at(
        mut self,
        deadline: Option<Instant>,
    ) -> Result<(), IrohNodeShutdownError> {
        let started = Instant::now();
        let watchdog = started + ROUTER_WATCHDOG;
        let watchdog = deadline.map_or(watchdog, |deadline| deadline.min(watchdog));
        let mut failures = Vec::new();
        if let Some(handle) = self.net_recovery.take() {
            handle.abort();
            match handle.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => failures
                    .push(anyhow::Error::new(error).context("stop network recovery watchdog")),
            }
        }

        if let Err(source) = self.quiesce_session().await {
            warn!(error = %source, "iroh session protocols did not quiesce cleanly");
            failures.push(anyhow::Error::new(source).context("quiesce network session"));
        }

        if self.router.is_shutdown() {
            // 当前库不会再返回提前结束的 Router 结果，不能把这条路径当作正常关闭。
            failures.push(RouterStoppedBeforeShutdown.into());
        }
        // 首先轮询 Router.shutdown 取得唯一 join，再关闭 endpoint，避免 Router 先结束而丢失失败。
        let (result, observers) = tokio::join!(
            biased;
            async {
                let closing = self.router.shutdown();
                tokio::pin!(closing);
                match timeout_at(watchdog, &mut closing).await {
                    Ok(result) => result,
                    Err(_) => {
                        warn!(
                            budget_ms = watchdog.saturating_duration_since(started).as_millis() as u64,
                            "iroh router cleanup is still pending; retaining shutdown ownership"
                        );
                        closing.await
                    }
                }
            },
            async {
                self.session_context.endpoint.close().await;
                self.connection_observations.shutdown(deadline).await
            }
        );
        failures.extend(
            observers
                .into_iter()
                .map(|error| anyhow::Error::new(error).context("stop connection observer")),
        );
        if let Err(error) = result {
            failures.push(anyhow::Error::new(error).context("stop network router"));
        }
        debug!("iroh node shut down");
        if failures.is_empty() {
            Ok(())
        } else {
            Err(IrohNodeShutdownError { failures })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use iroh::endpoint::Connection;
    use iroh::protocol::{AcceptError, ProtocolHandler};
    use tokio::sync::Notify;
    use tokio::task::JoinError;
    use tokio::time::timeout;

    use super::super::super::protocol_router::ProtocolRouterBuilder;
    use super::super::{tests::identity_store, IrohNodeBuilder, IrohNodeConfig};
    use super::{Duration, Instant};

    #[derive(Debug)]
    struct HeldCleanup {
        started: Arc<Notify>,
        release: Arc<Notify>,
        resource: Arc<()>,
    }

    impl ProtocolHandler for HeldCleanup {
        async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
            Ok(())
        }

        async fn shutdown(&self) {
            let _held = Arc::clone(&self.resource);
            self.started.notify_one();
            self.release.notified().await;
        }
    }

    #[derive(Debug)]
    struct FailedCleanup(Arc<()>);

    impl ProtocolHandler for FailedCleanup {
        async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
            Ok(())
        }

        async fn shutdown(&self) {
            let _held = Arc::clone(&self.0);
            panic!("PRIVATE_ROUTER_FAILURE");
        }
    }

    #[tokio::test]
    async fn node_preserves_multiple_failures_and_still_releases_network_resources() {
        let store = identity_store();
        let mut builder = IrohNodeBuilder::bind(
            &store,
            IrohNodeConfig {
                disable_relays: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let resource = Arc::new(());
        let replacement = ProtocolRouterBuilder::new((*builder.session_context.endpoint).clone());
        let router_builder = std::mem::replace(&mut builder.router_builder, replacement);
        builder.router_builder =
            router_builder.accept(b"test/failed-cleanup", FailedCleanup(Arc::clone(&resource)));
        let mut node = builder.spawn();
        if let Some(task) = node.net_recovery.take() {
            task.abort();
            let _ = task.await;
        }
        let failed = tokio::spawn(async {
            panic!("PRIVATE_RECOVERY_FAILURE");
        });
        while !failed.is_finished() {
            tokio::task::yield_now().await;
        }
        node.net_recovery = Some(failed);
        let failure = node.shutdown().await.unwrap_err();
        assert_eq!(failure.failures().len(), 2);
        assert!(failure.failures()[0]
            .downcast_ref::<JoinError>()
            .unwrap()
            .is_panic());
        assert!(failure.source().is_some());
        assert!(!format!("{failure:?} {failure}").contains("PRIVATE_"));
        assert_eq!(Arc::strong_count(&resource), 1);
        IrohNodeBuilder::bind(
            &store,
            IrohNodeConfig {
                disable_relays: true,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .spawn()
        .shutdown()
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn router_that_already_exited_cannot_be_reported_as_clean_shutdown() {
        let store = identity_store();
        let node = IrohNodeBuilder::bind(
            &store,
            IrohNodeConfig {
                disable_relays: true,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .spawn();
        node.session_context.endpoint.close().await;
        timeout(Duration::from_secs(5), async {
            while !node.router.is_shutdown() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let error = node.shutdown().await.unwrap_err();
        assert!(error
            .failures()
            .iter()
            .any(|error| error.is::<super::RouterStoppedBeforeShutdown>()));
    }

    #[tokio::test]
    async fn slow_protocol_cleanup_keeps_node_owned_after_watchdog() {
        assert_cleanup_remains_owned(None, Duration::from_secs(7)).await;
    }

    #[tokio::test]
    async fn expired_host_deadline_does_not_abandon_protocol_cleanup() {
        assert_cleanup_remains_owned(Some(Duration::ZERO), Duration::from_millis(10)).await;
    }

    async fn assert_cleanup_remains_owned(budget: Option<Duration>, wait: Duration) {
        let store = identity_store();
        let mut builder = IrohNodeBuilder::bind(
            &store,
            IrohNodeConfig {
                disable_relays: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let resource = Arc::new(());
        let replacement = ProtocolRouterBuilder::new((*builder.session_context.endpoint).clone());
        let router_builder = std::mem::replace(&mut builder.router_builder, replacement);
        builder.router_builder = router_builder.accept(
            b"test/held-cleanup",
            HeldCleanup {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                resource: Arc::clone(&resource),
            },
        );
        let deadline = budget.map(|budget| Instant::now() + budget);
        let mut closing = tokio::spawn(builder.spawn().shutdown_at(deadline));
        timeout(Duration::from_secs(10), started.notified())
            .await
            .unwrap();
        let early = timeout(wait, &mut closing).await;
        let retained = Arc::strong_count(&resource) > 1;
        release.notify_one();
        if early.is_err() {
            timeout(Duration::from_secs(10), closing)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        assert!(
            early.is_err(),
            "watchdog 不能把协议资源仍被持有的节点报告为关闭完成"
        );
        assert!(retained);
        assert_eq!(Arc::strong_count(&resource), 1);
    }
}
