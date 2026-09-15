//! Preserve registration ownership while selecting the current wire version first.

use iroh::protocol::{DynProtocolHandler, IncomingFilterOutcome, Router, RouterBuilder};
use iroh::Endpoint;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::PEER_REACHABILITY_ALPN;

pub(super) struct ProtocolRouterBuilder {
    inner: RouterBuilder,
    alpns: Vec<Vec<u8>>,
}

impl ProtocolRouterBuilder {
    pub(super) fn new(endpoint: Endpoint) -> Self {
        Self {
            inner: Router::builder(endpoint),
            alpns: Vec::new(),
        }
    }

    pub(super) fn accept(
        mut self,
        alpn: impl AsRef<[u8]>,
        handler: impl Into<Box<dyn DynProtocolHandler>>,
    ) -> Self {
        let alpn = alpn.as_ref().to_vec();
        if !self.alpns.contains(&alpn) {
            self.alpns.push(alpn.clone());
        }
        self.inner = self.inner.accept(alpn, handler);
        self
    }

    pub(super) fn spawn(mut self) -> Router {
        // Router sorts ALPNs lexically, but TLS uses the server's preference order.
        self.alpns
            .sort_by_key(|alpn| alpn.as_slice() != PEER_REACHABILITY_ALPN);
        let ready = Arc::new(AtomicBool::new(false));
        self.inner = self.inner.incoming_filter(Arc::new({
            let ready = ready.clone();
            move |_| {
                if ready.load(Ordering::Acquire) {
                    IncomingFilterOutcome::Accept
                } else {
                    IncomingFilterOutcome::Retry
                }
            }
        }));
        let router = self.inner.spawn();
        router.endpoint().set_alpns(self.alpns);
        ready.store(true, Ordering::Release);
        router
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::iroh::session_generation::{
        SessionProtocolHandlersBuilder, SessionProtocolRegistry, SessionProtocolRegistryError,
    };
    use crate::network::iroh::LEGACY_PEER_REACHABILITY_ALPN;
    use iroh::{
        endpoint::{ConnectOptions, Connection, EndpointHooks},
        protocol::{AcceptError, ProtocolHandler},
        RelayMode,
    };
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tokio::sync::Notify;

    #[derive(Debug)]
    struct Hold;
    impl ProtocolHandler for Hold {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            connection.closed().await;
            Ok(())
        }
    }

    #[derive(Debug)]
    struct CountAndHold {
        accepted: Arc<AtomicUsize>,
        shutdowns: Arc<AtomicUsize>,
        started: Arc<Notify>,
    }

    impl ProtocolHandler for CountAndHold {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            self.accepted.fetch_add(1, AtomicOrdering::SeqCst);
            self.started.notify_one();
            connection.closed().await;
            Ok(())
        }

        async fn shutdown(&self) {
            self.shutdowns.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    #[tokio::test]
    async fn rejected_generation_handlers_are_closed_without_retiring_current_generation() {
        const TEST_ALPN: &[u8] = b"uniclipboard/test-rejected-generation/1";

        let registry = SessionProtocolRegistry::new();
        let active_shutdowns = Arc::new(AtomicUsize::new(0));
        let mut active_handlers = SessionProtocolHandlersBuilder::new();
        active_handlers
            .install(
                [TEST_ALPN],
                CountAndHold {
                    accepted: Arc::new(AtomicUsize::new(0)),
                    shutdowns: Arc::clone(&active_shutdowns),
                    started: Arc::new(Notify::new()),
                },
            )
            .unwrap();
        let active = registry.publish(active_handlers.build()).await.unwrap();

        let rejected_shutdowns = Arc::new(AtomicUsize::new(0));
        let mut rejected_handlers = SessionProtocolHandlersBuilder::new();
        rejected_handlers
            .install(
                [TEST_ALPN],
                CountAndHold {
                    accepted: Arc::new(AtomicUsize::new(0)),
                    shutdowns: Arc::clone(&rejected_shutdowns),
                    started: Arc::new(Notify::new()),
                },
            )
            .unwrap();
        let error = registry
            .publish(rejected_handlers.build())
            .await
            .expect_err("second generation must be rejected while the first is active");
        assert!(matches!(error, SessionProtocolRegistryError::AlreadyActive));

        assert_eq!(rejected_shutdowns.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(active_shutdowns.load(AtomicOrdering::SeqCst), 0);

        registry.quiesce(&active).await.unwrap();
        assert_eq!(active_shutdowns.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn published_generation_replaces_retired_handler_without_restarting_router() {
        const TEST_ALPN: &[u8] = b"uniclipboard/test-session-generation/1";

        let registry = Arc::new(SessionProtocolRegistry::new());
        let dispatcher = registry.dispatcher(TEST_ALPN);
        let hooks = registry.endpoint_hooks();
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .hooks(registry.endpoint_hooks())
            .bind()
            .await
            .unwrap();
        let client = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .unwrap();
        let router = ProtocolRouterBuilder::new(server.clone())
            .accept(TEST_ALPN, dispatcher)
            .spawn();

        assert!(matches!(
            hooks.before_connect(&server.addr(), TEST_ALPN).await,
            iroh::endpoint::BeforeConnectOutcome::Reject
        ));
        let unavailable = client.connect(server.addr(), TEST_ALPN).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), unavailable.closed())
            .await
            .unwrap();

        let first_count = Arc::new(AtomicUsize::new(0));
        let first_shutdowns = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(Notify::new());
        let mut first_handlers = SessionProtocolHandlersBuilder::new();
        first_handlers
            .install(
                [TEST_ALPN],
                CountAndHold {
                    accepted: Arc::clone(&first_count),
                    shutdowns: Arc::clone(&first_shutdowns),
                    started: Arc::clone(&first_started),
                },
            )
            .unwrap();
        let first = registry.publish(first_handlers.build()).await.unwrap();
        assert!(matches!(
            hooks.before_connect(&server.addr(), TEST_ALPN).await,
            iroh::endpoint::BeforeConnectOutcome::Accept
        ));
        let first_connection = client.connect(server.addr(), TEST_ALPN).await.unwrap();
        first_started.notified().await;

        registry.quiesce(&first).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), first_connection.closed())
            .await
            .unwrap();
        assert_eq!(first_shutdowns.load(AtomicOrdering::SeqCst), 1);
        assert!(matches!(
            hooks.before_connect(&server.addr(), TEST_ALPN).await,
            iroh::endpoint::BeforeConnectOutcome::Reject
        ));

        let between_generations = client.connect(server.addr(), TEST_ALPN).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            between_generations.closed(),
        )
        .await
        .unwrap();

        let second_count = Arc::new(AtomicUsize::new(0));
        let second_shutdowns = Arc::new(AtomicUsize::new(0));
        let second_started = Arc::new(Notify::new());
        let mut second_handlers = SessionProtocolHandlersBuilder::new();
        second_handlers
            .install(
                [TEST_ALPN],
                CountAndHold {
                    accepted: Arc::clone(&second_count),
                    shutdowns: Arc::clone(&second_shutdowns),
                    started: Arc::clone(&second_started),
                },
            )
            .unwrap();
        let second = registry.publish(second_handlers.build()).await.unwrap();
        let second_connection = client.connect(server.addr(), TEST_ALPN).await.unwrap();
        second_started.notified().await;

        assert_eq!(first_count.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(second_count.load(AtomicOrdering::SeqCst), 1);

        second_connection.close(0u32.into(), b"test_complete");
        registry.quiesce(&second).await.unwrap();
        assert_eq!(second_shutdowns.load(AtomicOrdering::SeqCst), 1);
        client.close().await;
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn new_peers_negotiate_current_version_and_legacy_peers_still_connect() {
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .unwrap();
        let client = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .unwrap();
        let router = ProtocolRouterBuilder::new(server.clone())
            .accept(LEGACY_PEER_REACHABILITY_ALPN, Hold)
            .accept(PEER_REACHABILITY_ALPN, Hold)
            .spawn();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while server.addr().addrs.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let current = client
            .connect_with_opts(
                server.addr(),
                PEER_REACHABILITY_ALPN,
                ConnectOptions::new()
                    .with_additional_alpns(vec![LEGACY_PEER_REACHABILITY_ALPN.to_vec()]),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        assert_eq!(current.alpn(), PEER_REACHABILITY_ALPN);
        let legacy = client
            .connect(server.addr(), LEGACY_PEER_REACHABILITY_ALPN)
            .await
            .unwrap();
        assert_eq!(legacy.alpn(), LEGACY_PEER_REACHABILITY_ALPN);
        client.close().await;
        router.shutdown().await.unwrap();
    }
}
