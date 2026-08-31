use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};

use iroh::endpoint::{
    AfterHandshakeOutcome, BeforeConnectOutcome, Connection, EndpointHooks, VarInt,
    WeakConnectionHandle,
};
use iroh::EndpointAddr;

#[derive(Clone, Default)]
pub struct IrohNetworkPartitionGate {
    state: Arc<Mutex<PartitionState>>,
}

#[derive(Default)]
struct PartitionState {
    local_endpoint_id: Option<[u8; 32]>,
    blocked: HashSet<[u8; 32]>,
    connections: Vec<([u8; 32], WeakConnectionHandle)>,
}

impl IrohNetworkPartitionGate {
    pub fn local_endpoint_id(&self) -> Option<[u8; 32]> {
        self.state().local_endpoint_id
    }

    pub fn replace_blocked(&self, blocked: impl IntoIterator<Item = [u8; 32]>) -> usize {
        let mut state = self.state();
        state.blocked = blocked.into_iter().collect();
        let blocked = state.blocked.clone();
        state.connections.retain(|(endpoint_id, weak)| {
            let Some(connection) = weak.upgrade() else {
                return false;
            };
            if blocked.contains(endpoint_id) {
                connection.close(VarInt::from_u32(0), b"test network partition");
                false
            } else {
                true
            }
        });
        state.blocked.len()
    }

    pub(crate) fn install_local_endpoint_id(&self, endpoint_id: [u8; 32]) {
        self.state().local_endpoint_id = Some(endpoint_id);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PartitionState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl fmt::Debug for IrohNetworkPartitionGate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        formatter
            .debug_struct("IrohNetworkPartitionGate")
            .field("has_local_endpoint_id", &state.local_endpoint_id.is_some())
            .field("blocked_peer_count", &state.blocked.len())
            .field("tracked_connection_count", &state.connections.len())
            .finish()
    }
}

impl EndpointHooks for IrohNetworkPartitionGate {
    async fn before_connect(
        &self,
        remote_addr: &EndpointAddr,
        _alpn: &[u8],
    ) -> BeforeConnectOutcome {
        if self.state().blocked.contains(remote_addr.id.as_bytes()) {
            BeforeConnectOutcome::Reject
        } else {
            BeforeConnectOutcome::Accept
        }
    }

    async fn after_handshake(&self, connection: &Connection) -> AfterHandshakeOutcome {
        let endpoint_id = *connection.remote_id().as_bytes();
        let mut state = self.state();
        if state.blocked.contains(&endpoint_id) {
            return AfterHandshakeOutcome::Reject {
                error_code: VarInt::from_u32(0),
                reason: b"test network partition".to_vec(),
            };
        }
        state
            .connections
            .push((endpoint_id, connection.weak_handle()));
        AfterHandshakeOutcome::Accept
    }
}
