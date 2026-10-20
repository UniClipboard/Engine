use super::{
    model::{Attempt, Retained},
    owner::Runner,
    PeerReachabilityTransportError,
};
use crate::connectivity::{ConnectionLease, ConnectivityError, PendingInboundLease};
use crate::space::KnownPeerContact;
use chrono::{TimeZone, Utc};
use std::sync::atomic::Ordering;
use std::sync::PoisonError;
use tokio::time::Instant;
use uc_core::error_class::ErrorClass;
use uc_core::ports::PeerReachabilityChanged;
use uc_core::ports::ReachabilityState;
use uc_observability_contract::diagnostics::connectivity::{
    record_inbound_peer_rejection, InboundPeerProtocol, InboundPeerRejectionReason,
};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

impl Runner {
    pub async fn receive(
        &self,
        mut pending: PendingInboundLease,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityTransportError> {
        pending.deadline = deadline;
        let generation = {
            let state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.accepting || state.terminated {
                return Err(PeerReachabilityTransportError::Stopped);
            }
            state.generation
        };
        if pending.connection().is_closed() {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        self.ports
            .wire
            .read_admission(&mut *pending.receive, deadline)
            .await?;
        let peer = match self
            .ports
            .environment
            .identify(&pending.connection().authenticated_key())
            .await
        {
            Ok(peer) => peer,
            Err(ConnectivityError::Denied) => {
                self.ports
                    .wire
                    .write_confirmation(&mut *pending.send, false, deadline)
                    .await?;
                return Ok(());
            }
            Err(source) => return Err(source.into()),
        };
        if Instant::now() >= deadline
            || pending.connection().is_closed()
            || self
                .evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .generation
                != generation
        {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        pending.reserve_peer(peer)?;
        let attempt = self
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .begin(peer);
        if !self.authorize(&attempt, deadline).await? {
            record_inbound_peer_rejection(
                InboundPeerProtocol::Presence,
                InboundPeerRejectionReason::LedgerDenied,
            );
            let _ = self.contacts.send(KnownPeerContact { device_id: peer });
            self.ports
                .wire
                .write_confirmation(&mut *pending.send, false, deadline)
                .await?;
            return Ok(());
        }
        if Instant::now() >= deadline
            || !self
                .evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .current(&attempt)
        {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        let grant = pending.admit().await?;
        if Instant::now() >= deadline
            || !self
                .evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .current(&attempt)
        {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        let (lease, mut stream) = pending.bind(grant).await?;
        if !self
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .current(&attempt)
        {
            lease.close();
            return Err(PeerReachabilityTransportError::Stopped);
        }
        self.ports
            .wire
            .write_confirmation(&mut *stream.send, true, deadline)
            .await?;
        lease.check_current().await?;
        if Instant::now() >= deadline || attempt.stop.is_cancelled() {
            lease.close();
            return Err(PeerReachabilityTransportError::Stopped);
        }
        self.promote(&attempt, lease.clone(), true)?;
        // 初轮 FIN 与证据提交后才释放暂态额度；保留集合继续持有强租约。
        drop(stream);
        self.watch(attempt, lease);
        Ok(())
    }
    pub fn promote(
        &self,
        attempt: &Attempt,
        lease: ConnectionLease,
        inbound: bool,
    ) -> Result<(), PeerReachabilityTransportError> {
        let mut state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.current(attempt) || lease.is_closed() {
            lease.close();
            return Err(PeerReachabilityTransportError::Stopped);
        }
        let mut previous: Vec<(u64, usize)> = state
            .connections
            .iter()
            .filter(|(_, entry)| entry.lease.peer() == attempt.peer && entry.inbound == inbound)
            .map(|(id, entry)| (entry.promotion_order, *id))
            .collect();
        previous.sort_unstable();
        while previous.len() >= if inbound { 2 } else { 1 } {
            let (_, id) = previous.remove(0);
            if let Some(old) = state.connections.remove(&id) {
                old.lease.close();
            }
            state.verified.remove(&id);
        }
        let id = lease.connection_id();
        let promotion_order = state.revision;
        state.connections.insert(
            id,
            Retained {
                lease,
                inbound,
                promotion_order,
            },
        );
        state.verified.insert(id, Instant::now());
        state.succeeded(attempt.peer);
        let fresh_during_check = inbound
            && state.states.get(&attempt.peer) == Some(&ReachabilityState::Online)
            && (attempt.verification_was_active
                || attempt.epoch.check.try_lock().is_err()
                || attempt.epoch.revision.load(Ordering::Acquire) != attempt.verification_revision);
        self.publish(&mut state, attempt.peer, ReachabilityState::Online);
        if fresh_during_check {
            let at = Utc
                .timestamp_millis_opt(self.clock.now_ms())
                .single()
                .unwrap_or_else(Utc::now);
            let _ = self.events.send(PeerReachabilityChanged {
                device_id: attempt.peer,
                state: ReachabilityState::Online,
                at,
            });
        }
        Ok(())
    }
    pub fn record_failure(source: &PeerReachabilityTransportError) {
        if matches!(
            source,
            PeerReachabilityTransportError::Stopped
                | PeerReachabilityTransportError::Connection(ConnectivityError::Stopped)
        ) {
            uc_debug!(error_class = source.class(), "presence work stopped");
            return;
        }
        uc_warn!(
            error_class = source.class(),
            io_error_kind = io_error_kind(source),
            "presence exchange failed"
        );
    }
}
