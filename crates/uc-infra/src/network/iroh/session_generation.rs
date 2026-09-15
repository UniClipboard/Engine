use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use iroh::endpoint::{
    AfterHandshakeOutcome, BeforeConnectOutcome, Connection, EndpointHooks, WeakConnectionHandle,
};
use iroh::protocol::{AcceptError, DynProtocolHandler, ProtocolHandler};
use tokio::sync::{watch, Notify};

#[derive(Debug)]
pub(super) struct SessionProtocolRegistry {
    state: Mutex<RegistryState>,
}

#[derive(Debug)]
struct RegistryState {
    managed_alpns: BTreeSet<Vec<u8>>,
    current: Option<Arc<SessionProtocolGeneration>>,
}

#[derive(Debug)]
struct SessionProtocolGeneration {
    handlers: SessionProtocolHandlers,
    state: Mutex<GenerationState>,
    cancellation: watch::Sender<bool>,
    drained: Notify,
}

#[derive(Debug)]
struct GenerationState {
    phase: GenerationPhase,
    handler_leases: HashMap<usize, Connection>,
    connections: HashMap<usize, WeakConnectionHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GenerationPhase {
    Active,
    Draining,
    Retired,
}

#[derive(Debug)]
pub(super) struct SessionProtocolHandlers {
    routes: BTreeMap<Vec<u8>, usize>,
    handlers: Vec<Arc<dyn DynProtocolHandler>>,
}

#[derive(Debug)]
pub(super) struct SessionProtocolHandlersBuilder {
    routes: BTreeMap<Vec<u8>, usize>,
    handlers: Vec<Arc<dyn DynProtocolHandler>>,
}

#[derive(Debug)]
pub(super) struct SessionProtocolGenerationHandle {
    generation: Arc<SessionProtocolGeneration>,
}

#[derive(Debug)]
pub(super) struct SessionProtocolDispatcher {
    registry: Arc<SessionProtocolRegistry>,
    alpn: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct SessionProtocolEndpointHooks {
    registry: Arc<SessionProtocolRegistry>,
}

struct SessionProtocolLease {
    generation: Arc<SessionProtocolGeneration>,
    handler: Arc<dyn DynProtocolHandler>,
    lease_id: usize,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SessionProtocolRegistryError {
    #[error("a session protocol generation is already active")]
    AlreadyActive,
    #[error("the session protocol generation is not current")]
    NotCurrent,
    #[error("the session protocol generation is draining")]
    Draining,
    #[error("no session protocol generation is active")]
    Unavailable,
    #[error("the active session does not provide the requested protocol")]
    ProtocolUnavailable,
    #[error("the session protocol is already installed")]
    DuplicateProtocol,
}

#[derive(Debug, thiserror::Error)]
#[error("session protocol unavailable")]
struct SessionProtocolUnavailable;

impl SessionProtocolRegistry {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                managed_alpns: BTreeSet::new(),
                current: None,
            }),
        }
    }

    pub(super) fn dispatcher(
        self: &Arc<Self>,
        alpn: impl AsRef<[u8]>,
    ) -> SessionProtocolDispatcher {
        let alpn = alpn.as_ref().to_vec();
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .managed_alpns
            .insert(alpn.clone());
        SessionProtocolDispatcher {
            registry: Arc::clone(self),
            alpn,
        }
    }

    pub(super) fn endpoint_hooks(self: &Arc<Self>) -> SessionProtocolEndpointHooks {
        SessionProtocolEndpointHooks {
            registry: Arc::clone(self),
        }
    }

    pub(super) async fn publish(
        &self,
        handlers: SessionProtocolHandlers,
    ) -> Result<SessionProtocolGenerationHandle, SessionProtocolRegistryError> {
        let rejected_handlers = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.current.is_some() {
                handlers
            } else {
                let generation = Arc::new(SessionProtocolGeneration::new(handlers));
                state.current = Some(Arc::clone(&generation));
                return Ok(SessionProtocolGenerationHandle { generation });
            }
        };

        rejected_handlers.shutdown().await;
        Err(SessionProtocolRegistryError::AlreadyActive)
    }

    pub(super) async fn quiesce(
        &self,
        handle: &SessionProtocolGenerationHandle,
    ) -> Result<(), SessionProtocolRegistryError> {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(current) = state.current.as_ref() else {
                return Err(SessionProtocolRegistryError::NotCurrent);
            };
            if !Arc::ptr_eq(current, &handle.generation) {
                return Err(SessionProtocolRegistryError::NotCurrent);
            }
            state.current = None;
        }

        let connections = handle.generation.begin_quiesce();
        let closed = connections
            .iter()
            .map(WeakConnectionHandle::closed)
            .collect::<Vec<_>>();
        for connection in connections.iter().filter_map(WeakConnectionHandle::upgrade) {
            connection.close(0u32.into(), b"session_retired");
        }
        handle.generation.wait_until_drained().await;
        futures_util::future::join_all(closed).await;
        handle.generation.shutdown_handlers().await;
        handle.generation.mark_retired();
        Ok(())
    }

    fn acquire(
        &self,
        alpn: &[u8],
        connection: &Connection,
    ) -> Result<SessionProtocolLease, SessionProtocolRegistryError> {
        let generation = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current
            .clone()
            .ok_or(SessionProtocolRegistryError::Unavailable)?;
        generation.acquire(alpn, connection)
    }

    fn permits_outbound(&self, alpn: &[u8]) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.managed_alpns.contains(alpn) {
            return true;
        }
        state
            .current
            .as_ref()
            .is_some_and(|generation| generation.accepts(alpn))
    }

    fn register_connection(&self, connection: &Connection) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.managed_alpns.contains(connection.alpn()) {
            return true;
        }
        state
            .current
            .as_ref()
            .is_some_and(|generation| generation.register_connection(connection.alpn(), connection))
    }
}

impl SessionProtocolGeneration {
    fn new(handlers: SessionProtocolHandlers) -> Self {
        let (cancellation, _) = watch::channel(false);
        Self {
            handlers,
            state: Mutex::new(GenerationState {
                phase: GenerationPhase::Active,
                handler_leases: HashMap::new(),
                connections: HashMap::new(),
            }),
            cancellation,
            drained: Notify::new(),
        }
    }

    fn acquire(
        self: &Arc<Self>,
        alpn: &[u8],
        connection: &Connection,
    ) -> Result<SessionProtocolLease, SessionProtocolRegistryError> {
        let handler = self
            .handlers
            .handler(alpn)
            .ok_or(SessionProtocolRegistryError::ProtocolUnavailable)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.phase != GenerationPhase::Active {
            return Err(SessionProtocolRegistryError::Draining);
        }
        let lease_id = connection.stable_id();
        state.handler_leases.insert(lease_id, connection.clone());
        Ok(SessionProtocolLease {
            generation: Arc::clone(self),
            handler,
            lease_id,
        })
    }

    fn accepts(&self, alpn: &[u8]) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .phase
            == GenerationPhase::Active
            && self.handlers.routes.contains_key(alpn)
    }

    fn register_connection(&self, alpn: &[u8], connection: &Connection) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.phase != GenerationPhase::Active || !self.handlers.routes.contains_key(alpn) {
            return false;
        }
        state
            .connections
            .retain(|_, connection| connection.upgrade().is_some());
        state
            .connections
            .insert(connection.stable_id(), connection.weak_handle());
        true
    }

    fn begin_quiesce(&self) -> Vec<WeakConnectionHandle> {
        let connections = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.phase = GenerationPhase::Draining;
            state.connections.values().cloned().collect::<Vec<_>>()
        };
        self.cancellation.send_replace(true);
        connections
    }

    async fn wait_until_drained(&self) {
        loop {
            let notified = self.drained.notified();
            if self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .handler_leases
                .is_empty()
            {
                return;
            }
            notified.await;
        }
    }

    async fn shutdown_handlers(&self) {
        for handler in &self.handlers.handlers {
            handler.shutdown().await;
        }
    }

    fn mark_retired(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .phase = GenerationPhase::Retired;
    }
}

impl SessionProtocolHandlers {
    pub(super) async fn shutdown(self) {
        for handler in self.handlers {
            handler.shutdown().await;
        }
    }

    fn handler(&self, alpn: &[u8]) -> Option<Arc<dyn DynProtocolHandler>> {
        self.routes
            .get(alpn)
            .and_then(|index| self.handlers.get(*index))
            .cloned()
    }
}

impl SessionProtocolHandlersBuilder {
    pub(super) fn new() -> Self {
        Self {
            routes: BTreeMap::new(),
            handlers: Vec::new(),
        }
    }

    pub(super) fn install<I, A>(
        &mut self,
        alpns: I,
        handler: impl Into<Box<dyn DynProtocolHandler>>,
    ) -> Result<(), SessionProtocolRegistryError>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<[u8]>,
    {
        let alpns = alpns
            .into_iter()
            .map(|alpn| alpn.as_ref().to_vec())
            .collect::<Vec<_>>();
        if alpns.iter().any(|alpn| self.routes.contains_key(alpn)) {
            return Err(SessionProtocolRegistryError::DuplicateProtocol);
        }

        let handler_index = self.handlers.len();
        self.handlers.push(Arc::from(handler.into()));
        self.routes
            .extend(alpns.into_iter().map(|alpn| (alpn, handler_index)));
        Ok(())
    }

    pub(super) fn build(self) -> SessionProtocolHandlers {
        SessionProtocolHandlers {
            routes: self.routes,
            handlers: self.handlers,
        }
    }
}

impl ProtocolHandler for SessionProtocolDispatcher {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let lease = self
            .registry
            .acquire(&self.alpn, &connection)
            .map_err(|_| AcceptError::from_err(SessionProtocolUnavailable))?;
        let handler = Arc::clone(&lease.handler);
        let cancellation = lease.cancelled();

        tokio::select! {
            biased;
            _ = cancellation => {
                connection.close(0u32.into(), b"session_retired");
                Err(AcceptError::from_err(SessionProtocolUnavailable))
            }
            result = handler.accept(connection.clone()) => result,
        }
    }
}

impl EndpointHooks for SessionProtocolEndpointHooks {
    async fn before_connect(
        &self,
        _remote_addr: &iroh::EndpointAddr,
        alpn: &[u8],
    ) -> BeforeConnectOutcome {
        if self.registry.permits_outbound(alpn) {
            BeforeConnectOutcome::Accept
        } else {
            BeforeConnectOutcome::Reject
        }
    }

    async fn after_handshake(&self, connection: &Connection) -> AfterHandshakeOutcome {
        if self.registry.register_connection(connection) {
            AfterHandshakeOutcome::Accept
        } else {
            AfterHandshakeOutcome::Reject {
                error_code: 0u32.into(),
                reason: b"session_unavailable".to_vec(),
            }
        }
    }
}

impl SessionProtocolLease {
    async fn cancelled(&self) {
        let mut cancellation = self.generation.cancellation.subscribe();
        if *cancellation.borrow_and_update() {
            return;
        }
        let _ = cancellation.changed().await;
    }
}

impl Drop for SessionProtocolLease {
    fn drop(&mut self) {
        let mut state = self
            .generation
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.handler_leases.remove(&self.lease_id);
        if state.handler_leases.is_empty() {
            self.generation.drained.notify_waiters();
        }
    }
}
