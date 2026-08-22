use std::sync::Arc;

use super::{RePairingStateError, RePairingStateStorePort};

pub(crate) struct RePairingState {
    store: Arc<dyn RePairingStateStorePort>,
}

impl RePairingState {
    pub(crate) fn new(store: Arc<dyn RePairingStateStorePort>) -> Self {
        Self { store }
    }

    pub(crate) async fn is_required(&self) -> Result<bool, RePairingStateError> {
        self.store.is_required().await
    }

    pub(crate) async fn require_after_relationship_reset(&self) -> Result<(), RePairingStateError> {
        self.store.set_required(true).await
    }

    pub(crate) async fn resolve_after_successful_pairing(&self) -> Result<(), RePairingStateError> {
        self.store.set_required(false).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    #[derive(Default)]
    struct InMemoryRePairingStateStore {
        required: Mutex<bool>,
    }

    #[async_trait]
    impl RePairingStateStorePort for InMemoryRePairingStateStore {
        async fn is_required(&self) -> Result<bool, RePairingStateError> {
            Ok(*self.required.lock().unwrap())
        }

        async fn set_required(&self, required: bool) -> Result<(), RePairingStateError> {
            *self.required.lock().unwrap() = required;
            Ok(())
        }
    }

    #[tokio::test]
    async fn relationship_reset_requires_pairing() {
        let state = RePairingState::new(Arc::new(InMemoryRePairingStateStore::default()));

        state.require_after_relationship_reset().await.unwrap();

        assert!(state.is_required().await.unwrap());
    }

    #[tokio::test]
    async fn successful_pairing_resolves_requirement() {
        let store = Arc::new(InMemoryRePairingStateStore::default());
        *store.required.lock().unwrap() = true;
        let state = RePairingState::new(store);

        state.resolve_after_successful_pairing().await.unwrap();

        assert!(!state.is_required().await.unwrap());
    }
}
