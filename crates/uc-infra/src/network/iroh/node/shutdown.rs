use std::time::Duration;

use tokio::time::timeout;
use tracing::{debug, instrument, warn};

use super::IrohNode;

const ROUTER_WATCHDOG: Duration = Duration::from_secs(5);

impl IrohNode {
    /// 等待节点与协议处理器实际关闭；watchdog 只诊断慢收尾，不丢弃持有资源的 future。
    #[instrument(skip_all)]
    pub async fn shutdown(self) {
        if let Some(handle) = self.net_recovery {
            handle.abort();
            match handle.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(_) => warn!(
                    error_kind = "join_failed",
                    "net-recovery watchdog panicked before shutdown"
                ),
            }
        }

        self.endpoint.close().await;
        self.connection_observations.shutdown().await;

        // Router 首次 shutdown 会取出其任务；超时后重新调用会提前成功，必须继续等待同一个 future。
        let closing = self.router.shutdown();
        tokio::pin!(closing);
        let result = match timeout(ROUTER_WATCHDOG, &mut closing).await {
            Ok(result) => result,
            Err(_) => {
                warn!(
                    budget_ms = ROUTER_WATCHDOG.as_millis() as u64,
                    "iroh router cleanup is still pending; retaining shutdown ownership"
                );
                closing.await
            }
        };
        if result.is_err() {
            warn!(
                error_kind = "join_failed",
                "iroh router task joined with error"
            );
        }
        debug!("iroh node shut down");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iroh::endpoint::Connection;
    use iroh::protocol::{AcceptError, ProtocolHandler};
    use tokio::sync::Notify;

    use super::super::{tests::identity_store, IrohNodeBuilder, IrohNodeConfig};
    use super::{timeout, Duration};

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

    #[tokio::test]
    async fn slow_protocol_cleanup_keeps_node_owned_after_watchdog() {
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
        builder.router_builder = Some(builder.take_router_builder().unwrap().accept(
            b"test/held-cleanup",
            HeldCleanup {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                resource: Arc::clone(&resource),
            },
        ));
        let mut closing = tokio::spawn(builder.spawn().shutdown());
        timeout(Duration::from_secs(10), started.notified())
            .await
            .unwrap();
        let early = timeout(Duration::from_secs(7), &mut closing).await;
        let retained = Arc::strong_count(&resource) > 1;
        release.notify_one();
        if early.is_err() {
            timeout(Duration::from_secs(10), closing)
                .await
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
