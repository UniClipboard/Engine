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
    #[cfg(any(test, feature = "test-util"))]
    rejected_dials: HashSet<([u8; 32], Vec<u8>)>,
    #[cfg(any(test, feature = "test-util"))]
    rejected_dial_count: u64,
    #[cfg(any(test, feature = "test-util"))]
    suppress_opportunities: bool,
    #[cfg(any(test, feature = "test-util"))]
    admitted_transport_count: u64,
}

impl IrohNetworkPartitionGate {
    #[cfg(any(test, feature = "test-util"))]
    pub fn peer_reachability_connections(&self, retain_one: bool) -> (usize, usize, u64) {
        let state = self.state();
        let mut incoming = 0;
        let mut outgoing = 0;
        for (_, weak) in &state.connections {
            let Some(connection) = weak.upgrade() else {
                continue;
            };
            if connection.close_reason().is_some()
                || ![
                    super::PEER_REACHABILITY_ALPN,
                    super::LEGACY_PEER_REACHABILITY_ALPN,
                ]
                .contains(&connection.alpn())
            {
                continue;
            }
            if retain_one && incoming + outgoing > 0 {
                connection.close(VarInt::from_u32(0), b"test single connection topology");
                continue;
            }
            match connection.side() {
                iroh::endpoint::Side::Client => outgoing += 1,
                iroh::endpoint::Side::Server => incoming += 1,
            }
        }
        (incoming, outgoing, state.admitted_transport_count)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn suppress_connectivity_opportunities(&self, suppressed: bool) {
        self.state().suppress_opportunities = suppressed;
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn opportunities_suppressed(&self) -> bool {
        self.state().suppress_opportunities
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn reject_new_connections(&self, peers: Vec<[u8; 32]>, alpns: Vec<Vec<u8>>) -> usize {
        let mut state = self.state();
        state.rejected_dials = peers
            .iter()
            .flat_map(|peer| alpns.iter().map(move |alpn| (*peer, alpn.clone())))
            .collect();
        state.rejected_dial_count = 0;
        peers.len()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn rejected_dial_count(&self) -> u64 {
        self.state().rejected_dial_count
    }

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
        #[cfg(any(test, feature = "test-util"))]
        {
            let mut state = self.state();
            if state
                .rejected_dials
                .contains(&(*remote_addr.id.as_bytes(), _alpn.to_vec()))
            {
                state.rejected_dial_count += 1;
                return BeforeConnectOutcome::Reject;
            }
        }
        if self.state().blocked.contains(remote_addr.id.as_bytes()) {
            BeforeConnectOutcome::Reject
        } else {
            BeforeConnectOutcome::Accept
        }
    }

    async fn after_handshake(&self, connection: &Connection) -> AfterHandshakeOutcome {
        let endpoint_id = *connection.remote_id().as_bytes();
        let mut state = self.state();
        state
            .connections
            .retain(|(_, connection)| connection.upgrade().is_some());
        if state.blocked.contains(&endpoint_id) {
            return AfterHandshakeOutcome::Reject {
                error_code: VarInt::from_u32(0),
                reason: b"test network partition".to_vec(),
            };
        }
        #[cfg(any(test, feature = "test-util"))]
        if [
            super::PEER_REACHABILITY_ALPN,
            super::LEGACY_PEER_REACHABILITY_ALPN,
        ]
        .contains(&connection.alpn())
        {
            state.admitted_transport_count += 1;
        }
        state
            .connections
            .push((endpoint_id, connection.weak_handle()));
        AfterHandshakeOutcome::Accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{Endpoint, RelayMode};
    use std::time::Duration;

    #[tokio::test]
    async fn rejecting_new_dials_keeps_established_streams_usable() {
        const ALPN: &[u8] = b"test/reachability-gate";
        let gate = IrohNetworkPartitionGate::default();
        let a = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .hooks(gate.clone())
            .bind()
            .await
            .unwrap();
        let b = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while b.addr().addrs.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (outgoing, incoming) = tokio::join!(a.connect(b.addr(), ALPN), async {
            b.accept().await.unwrap().await.unwrap()
        });
        let outgoing = outgoing.unwrap();
        gate.reject_new_connections(vec![*b.id().as_bytes()], vec![ALPN.to_vec()]);
        assert!(a.connect(b.addr(), ALPN).await.is_err());
        assert_eq!(gate.rejected_dial_count(), 1);
        tokio::time::timeout(Duration::from_secs(2), async {
            let sender = async {
                let (mut send, mut receive) = outgoing.open_bi().await.unwrap();
                send.write_all(b"challenge").await.unwrap();
                send.finish().unwrap();
                assert_eq!(receive.read_to_end(9).await.unwrap(), b"challenge");
            };
            let receiver = async {
                let (mut send, mut receive) = incoming.accept_bi().await.unwrap();
                let bytes = receive.read_to_end(9).await.unwrap();
                send.write_all(&bytes).await.unwrap();
                send.finish().unwrap();
            };
            tokio::join!(sender, receiver);
        })
        .await
        .unwrap();
        assert!(outgoing.close_reason().is_none());
        a.close().await;
        b.close().await;
    }
}
