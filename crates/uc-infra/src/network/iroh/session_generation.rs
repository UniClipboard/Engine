use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, DynProtocolHandler, ProtocolHandler};
use tokio::sync::{watch, Notify};

#[derive(Debug)]
pub(super) struct SessionProtocolRegistry {
    state: Mutex<RegistryState>,
}

#[derive(Debug)]
struct RegistryState {
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
    leases: HashMap<usize, Connection>,
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
pub(super) struct SessionProtocolGenerationHandle {
    generation: Arc<SessionProtocolGeneration>,
}

#[derive(Debug)]
pub(super) struct SessionProtocolDispatcher {
    registry: Arc<SessionProtocolRegistry>,
    alpn: Vec<u8>,
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
}

#[derive(Debug, thiserror::Error)]
#[error("session protocol unavailable")]
struct SessionProtocolUnavailable;

impl SessionProtocolRegistry {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState { current: None }),
        }
    }

    pub(super) fn dispatcher(
        self: &Arc<Self>,
        alpn: impl AsRef<[u8]>,
    ) -> SessionProtocolDispatcher {
        SessionProtocolDispatcher {
            registry: Arc::clone(self),
            alpn: alpn.as_ref().to_vec(),
        }
    }

    pub(super) fn publish(
        &self,
        handlers: SessionProtocolHandlers,
    ) -> Result<SessionProtocolGenerationHandle, SessionProtocolRegistryError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.current.is_some() {
            return Err(SessionProtocolRegistryError::AlreadyActive);
        }

        let generation = Arc::new(SessionProtocolGeneration::new(handlers));
        state.current = Some(Arc::clone(&generation));
        Ok(SessionProtocolGenerationHandle { generation })
    }

    pub(super) async fn quiesce(
        &self,
        handle: SessionProtocolGenerationHandle,
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

        handle.generation.begin_quiesce();
        handle.generation.wait_until_drained().await;
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
}

impl SessionProtocolGeneration {
    fn new(handlers: SessionProtocolHandlers) -> Self {
        let (cancellation, _) = watch::channel(false);
        Self {
            handlers,
            state: Mutex::new(GenerationState {
                phase: GenerationPhase::Active,
                leases: HashMap::new(),
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
        state.leases.insert(lease_id, connection.clone());
        Ok(SessionProtocolLease {
            generation: Arc::clone(self),
            handler,
            lease_id,
        })
    }

    fn begin_quiesce(&self) {
        let connections = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.phase = GenerationPhase::Draining;
            state.leases.values().cloned().collect::<Vec<_>>()
        };
        self.cancellation.send_replace(true);
        for connection in connections {
            connection.close(0u32.into(), b"session_retired");
        }
    }

    async fn wait_until_drained(&self) {
        loop {
            let notified = self.drained.notified();
            if self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .leases
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
    #[cfg(test)]
    pub(super) fn single_for_test(alpn: impl AsRef<[u8]>, handler: impl ProtocolHandler) -> Self {
        let handler: Arc<dyn DynProtocolHandler> = Arc::new(handler);
        Self {
            routes: BTreeMap::from([(alpn.as_ref().to_vec(), 0)]),
            handlers: vec![handler],
        }
    }

    fn handler(&self, alpn: &[u8]) -> Option<Arc<dyn DynProtocolHandler>> {
        self.routes
            .get(alpn)
            .and_then(|index| self.handlers.get(*index))
            .cloned()
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
        state.leases.remove(&self.lease_id);
        if state.leases.is_empty() {
            self.generation.drained.notify_waiters();
        }
    }
}
