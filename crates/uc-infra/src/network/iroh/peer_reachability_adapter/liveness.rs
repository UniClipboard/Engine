//! Liveness checks on existing admitted peer connections.

use futures_util::{stream::FuturesUnordered, StreamExt};

use super::{
    peer_reachability_protocol, AttemptObservation, Connection, DeviceId, HandlerState, Instant,
    IrohPeerReachabilityAdapter, Ordering, PeerReachabilityError, ReachabilityState,
    LEGACY_PEER_REACHABILITY_ALPN,
};
use peer_reachability_protocol::CheckResult;

impl HandlerState {
    pub(super) async fn serve_liveness(&self, device: DeviceId, connection: &Connection) {
        if connection.alpn() == LEGACY_PEER_REACHABILITY_ALPN {
            connection.closed().await;
            return;
        }
        let epoch = self.observations.lock().await.begin(device);
        let mut requests = FuturesUnordered::new();
        loop {
            tokio::select! {
                biased;
                _ = connection.closed() => break,
                _ = requests.next(), if !requests.is_empty() => {},
                streams = connection.accept_bi() => {
                    let Ok((mut send, mut receive)) = streams else { break };
                    if requests.len() >= 2 {
                        let _ = send.reset(0u32.into());
                        let _ = receive.stop(0u32.into());
                        continue;
                    }
                    let epoch = epoch.clone();
                    requests.push(async move {
                        let exchange = async {
                            let Ok(bytes) = receive.read_to_end(peer_reachability_protocol::FRAME_SIZE).await else { return };
                            let Some(challenge) = peer_reachability_protocol::challenge(&bytes) else { return };
                            let admitted = self.is_admitted(&device).await;
                            let current = self.observations.lock().await.is_current(device, &epoch)
                                && self.accepting.load(Ordering::Acquire);
                            if send.write_all(&peer_reachability_protocol::reply(&challenge, admitted && current)).await.is_ok() {
                                let _ = send.finish();
                            }
                        };
                        let _ = tokio::time::timeout(peer_reachability_protocol::CHECK_BUDGET, exchange).await;
                    });
                }
            }
        }
    }
}

impl IrohPeerReachabilityAdapter {
    pub(super) async fn check_established(
        &self,
        device: &DeviceId,
        before: &AttemptObservation,
        failure: &mut super::PresenceCheckResult,
    ) -> Result<Option<ReachabilityState>, PeerReachabilityError> {
        let admitted = self
            .handler_state
            .peer_admission
            .is_admitted(device)
            .await
            .map_err(PeerReachabilityError::internal)?;
        if !admitted {
            uc_core::ports::PeerReachabilityPort::forget(self, device).await;
            return Ok(Some(ReachabilityState::Offline));
        }
        let mut snapshots = Vec::new();
        {
            let observations = self.observations.lock().await;
            if !observations.is_current(*device, before)
                || !self.handler_state.accepting.load(Ordering::Acquire)
            {
                return Ok(Some(ReachabilityState::Offline));
            }
            if let Some(peer) = self.peers.lock().await.get(device) {
                snapshots.push(peer.connection.clone());
            }
            snapshots.extend(
                self.handler_state
                    .inbound_connections
                    .lock()
                    .await
                    .values()
                    .filter(|(id, _)| id == device)
                    .map(|(_, connection)| connection.clone()),
            );
        }
        if snapshots.is_empty() {
            let observations = self.observations.lock().await;
            if observations.is_current(*device, before)
                && observations.successes.get(device).copied() == before.success
            {
                let previous = self
                    .last_state
                    .lock()
                    .await
                    .insert(*device, ReachabilityState::Offline);
                if previous == Some(ReachabilityState::Online) {
                    self.broadcast(*device, ReachabilityState::Offline, self.now());
                }
            }
            return Ok(None);
        }
        let checks = FuturesUnordered::new();
        for connection in &snapshots {
            checks.push(async move {
                let result = if connection.close_reason().is_some() {
                    CheckResult::Failed
                } else if connection.alpn() == LEGACY_PEER_REACHABILITY_ALPN {
                    CheckResult::Alive
                } else {
                    peer_reachability_protocol::check(connection).await
                };
                (connection, result)
            });
        }
        let mut checks = checks;
        let mut alive_connections = Vec::new();
        let mut rejected_by_peer = false;
        while let Some((connection, result)) = checks.next().await {
            match result {
                CheckResult::Alive => alive_connections.push(connection),
                CheckResult::Rejected => {
                    rejected_by_peer = true;
                    *failure = super::PresenceCheckResult::Confirmation(
                        super::ConfirmationFailure::PeerNotAdmitted,
                    );
                }
                CheckResult::TimedOut if !rejected_by_peer => {
                    *failure = super::PresenceCheckResult::Confirmation(
                        super::ConfirmationFailure::TimedOut,
                    );
                }
                CheckResult::Failed if !rejected_by_peer => {
                    *failure = super::PresenceCheckResult::Confirmation(
                        super::ConfirmationFailure::TransportFailed,
                    );
                }
                CheckResult::TimedOut | CheckResult::Failed => {}
            }
        }
        if rejected_by_peer {
            alive_connections.clear();
        }
        for connection in alive_connections {
            let admitted = self
                .handler_state
                .peer_admission
                .is_admitted(device)
                .await
                .map_err(PeerReachabilityError::internal)?;
            let mut observations = self.observations.lock().await;
            if !admitted
                || !observations.is_current(*device, before)
                || !self.handler_state.accepting.load(Ordering::Acquire)
            {
                return Ok(Some(ReachabilityState::Offline));
            }
            if connection.close_reason().is_some() {
                continue;
            }
            let retained = self
                .peers
                .lock()
                .await
                .get(device)
                .is_some_and(|peer| peer.connection.stable_id() == connection.stable_id())
                || self
                    .handler_state
                    .inbound_connections
                    .lock()
                    .await
                    .contains_key(&connection.stable_id());
            if !retained {
                continue;
            }
            if connection.alpn() != LEGACY_PEER_REACHABILITY_ALPN {
                observations
                    .verified
                    .insert(connection.stable_id(), Instant::now());
                observations.succeeded(*device);
            }
            let previous = self
                .last_state
                .lock()
                .await
                .insert(*device, ReachabilityState::Online);
            if previous != Some(ReachabilityState::Online) {
                self.broadcast(*device, ReachabilityState::Online, self.now());
            }
            return Ok(Some(ReachabilityState::Online));
        }
        let mut observations = self.observations.lock().await;
        if !observations.is_current(*device, before)
            || observations.successes.get(device).copied() != before.success
        {
            return Ok(Some(
                self.last_state
                    .lock()
                    .await
                    .get(device)
                    .copied()
                    .unwrap_or(ReachabilityState::Unknown),
            ));
        }
        for connection in &snapshots {
            observations.verified.remove(&connection.stable_id());
            connection.close(0u32.into(), b"liveness_expired");
        }
        self.peers.lock().await.remove(device);
        self.handler_state
            .inbound_connections
            .lock()
            .await
            .retain(|_, (id, _)| id != device);
        let previous = self
            .last_state
            .lock()
            .await
            .insert(*device, ReachabilityState::Offline);
        if previous != Some(ReachabilityState::Offline) {
            self.broadcast(*device, ReachabilityState::Offline, self.now());
        }
        Ok(Some(ReachabilityState::Offline))
    }
}
