use super::{
    model::Attempt,
    owner::{Runner, FAST_PATH_TTL, STEP_BUDGET},
    PeerReachabilityTransportError,
};
use crate::connectivity::{ConnectionLease, ConnectionProtocol, ConnectivityError, StreamLease};
use futures::{stream::FuturesUnordered, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::PoisonError;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ports::ReachabilityState;
use uc_observability_contract::diagnostics::connectivity::{
    record_presence_check_result, ConfirmationFailure, DialFailure, PresenceCheckFeedback,
    PresenceCheckResult,
};
use uc_observability_contract::diagnostics::ObservationContext;

enum CheckOutcome {
    Alive,
    Rejected,
    Failed,
}

impl Runner {
    pub async fn verify(
        &self,
        attempt: Attempt,
        fresh: bool,
        deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<ReachabilityState, PeerReachabilityTransportError> {
        let peer = attempt.peer;
        if !self
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .current(&attempt)
        {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        let _check = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(PeerReachabilityTransportError::Stopped),
            result = timeout_at(deadline, attempt.epoch.check.lock()) => result.map_err(|source| PeerReachabilityTransportError::Connection(ConnectivityError::TimedOut(source)))?,
        };
        attempt.epoch.revision.fetch_add(1, Ordering::AcqRel);
        if !self
            .authorize_with_cancel(
                &attempt,
                deadline.min(Instant::now() + Duration::from_secs(1)),
                &cancel,
            )
            .await?
        {
            record_presence_check_result(PresenceCheckResult::AuthorityDenied);
            self.revoke(&attempt);
            return Ok(ReachabilityState::Offline);
        }
        let snapshots = {
            let state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
            if cancel.is_cancelled() {
                return Err(PeerReachabilityTransportError::Stopped);
            }
            if !state.current(&attempt) {
                return Ok(ReachabilityState::Offline);
            }
            if !fresh
                && state.connections.iter().any(|(id, entry)| {
                    entry.lease.peer() == peer
                        && !entry.lease.is_closed()
                        && state
                            .verified
                            .get(id)
                            .is_some_and(|at| at.elapsed() < FAST_PATH_TTL)
                })
            {
                record_presence_check_result(PresenceCheckResult::Reachable);
                return Ok(ReachabilityState::Online);
            }
            state
                .connections
                .values()
                .filter(|entry| entry.lease.peer() == peer)
                .map(|entry| entry.lease.clone())
                .collect::<Vec<_>>()
        };
        if !snapshots.is_empty() {
            let round = deadline.min(Instant::now() + STEP_BUDGET);
            let mut checks = FuturesUnordered::new();
            for lease in &snapshots {
                let cancel = &cancel;
                checks.push(async move { (lease, self.check(lease, round, cancel).await) });
            }
            let mut alive = Vec::new();
            let mut rejected = false;
            let mut local_error = None;
            while let Some((lease, result)) = checks.next().await {
                match result {
                    Ok(CheckOutcome::Alive) => alive.push(lease.clone()),
                    Ok(CheckOutcome::Rejected) => {
                        rejected = true;
                        lease.close();
                    }
                    Ok(CheckOutcome::Failed) => {}
                    Err(source) => {
                        if matches!(&source, PeerReachabilityTransportError::Connection(error) if error.is_admission_read_failure())
                        {
                            local_error = Some(source);
                            continue;
                        }
                        if cancel.is_cancelled() {
                            continue;
                        }
                        Self::record_failure(&source);
                        lease.close();
                    }
                }
            }
            if cancel.is_cancelled() {
                return Err(PeerReachabilityTransportError::Stopped);
            }
            if rejected {
                if let Some(source) = &local_error {
                    Self::record_failure(source);
                }
                for lease in &snapshots {
                    lease.close();
                }
                record_presence_check_result(PresenceCheckResult::Confirmation(
                    ConfirmationFailure::PeerNotAdmitted,
                ));
                return Ok(self.failed(&attempt));
            }
            if let Some(source) = local_error {
                record_presence_check_result(PresenceCheckResult::AuthorityUnavailable);
                self.local_failure(&attempt);
                return Err(source);
            }
            if !alive.is_empty() {
                if !self
                    .authorize_with_cancel(&attempt, deadline, &cancel)
                    .await?
                {
                    self.revoke(&attempt);
                    return Ok(ReachabilityState::Offline);
                }
                let mut state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
                if cancel.is_cancelled() {
                    return Err(PeerReachabilityTransportError::Stopped);
                }
                if !state.current(&attempt) {
                    return Ok(ReachabilityState::Offline);
                }
                for lease in alive {
                    if !lease.is_closed() && state.connections.contains_key(&lease.connection_id())
                    {
                        if lease.protocol() == ConnectionProtocol::PeerReachabilityV2 {
                            state.verified.insert(lease.connection_id(), Instant::now());
                        }
                        state.succeeded(peer);
                        self.publish(&mut state, peer, ReachabilityState::Online);
                        record_presence_check_result(PresenceCheckResult::Reachable);
                        return Ok(ReachabilityState::Online);
                    }
                }
            }
            self.failed(&attempt);
        }
        if cancel.is_cancelled() || Instant::now() >= deadline {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(PeerReachabilityTransportError::Stopped),
            _ = self.ports.environment.recover_for_demand() => {},
        }
        let connected = self
            .manager
            .connect_with_cancel(peer, deadline, &cancel)
            .await;
        let lease = match connected {
            Ok(lease) => lease,
            Err(source) if source.is_address_missing() => {
                record_presence_check_result(PresenceCheckResult::AddressMissing);
                self.failed(&attempt);
                return Err(source.into());
            }
            Err(source) if source.is_unavailable() => {
                record_presence_check_result(PresenceCheckResult::Dial(
                    DialFailure::TransportFailed,
                ));
                Self::record_failure(&source.into());
                return Ok(self.failed(&attempt));
            }
            Err(source) => return Err(source.into()),
        };
        let mut stream = lease
            .open_bi_with_cancel(deadline.min(Instant::now() + STEP_BUDGET), &cancel)
            .await?;
        let exchange = async {
            self.ports
                .wire
                .write_admission(&mut *stream.send, deadline)
                .await?;
            let admitted = self
                .ports
                .wire
                .read_confirmation(&mut *stream.receive, deadline)
                .await?;
            Ok::<_, PeerReachabilityTransportError>(admitted)
        };
        let exchanged = tokio::select! {
            biased;
            _ = cancel.cancelled() => { lease.close(); return Err(PeerReachabilityTransportError::Stopped); }
            result = exchange => result,
        };
        let accepted = match exchanged {
            Ok(result) => result,
            Err(source) => {
                lease.close();
                if matches!(&source, PeerReachabilityTransportError::Connection(error) if error.is_admission_read_failure())
                {
                    record_presence_check_result(PresenceCheckResult::AuthorityUnavailable);
                    self.local_failure(&attempt);
                    return Err(source);
                }
                record_presence_check_result(PresenceCheckResult::Confirmation(
                    ConfirmationFailure::TransportFailed,
                ));
                Self::record_failure(&source);
                return Ok(self.failed(&attempt));
            }
        };
        if !accepted {
            record_presence_check_result(PresenceCheckResult::Confirmation(
                ConfirmationFailure::PeerNotAdmitted,
            ));
            lease.close();
            return Ok(self.failed(&attempt));
        }
        lease.check_current().await?;
        if Instant::now() >= deadline || cancel.is_cancelled() {
            lease.close();
            return Err(PeerReachabilityTransportError::Stopped);
        }
        self.promote(&attempt, lease.clone(), false)?;
        drop(stream);
        self.watch(attempt.clone(), lease);
        record_presence_check_result(PresenceCheckResult::Reachable);
        Ok(ReachabilityState::Online)
    }
    async fn check(
        &self,
        lease: &ConnectionLease,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<CheckOutcome, PeerReachabilityTransportError> {
        if lease.is_closed() {
            return Ok(CheckOutcome::Failed);
        }
        if lease.protocol() == ConnectionProtocol::PeerReachabilityV1 {
            return Ok(CheckOutcome::Alive);
        }
        let nonce: [u8; 16] = rand::random();
        let mut stream = lease.open_bi_with_cancel(deadline, cancel).await?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(PeerReachabilityTransportError::Stopped),
            result = async {
                self.ports.wire.write_challenge(&mut *stream.send, &nonce, deadline).await?;
                Ok(if self.ports.wire.read_reply(&mut *stream.receive, &nonce, deadline).await? { CheckOutcome::Alive } else { CheckOutcome::Rejected })
            } => result,
        }
    }

    pub fn watch(&self, attempt: Attempt, lease: ConnectionLease) {
        let runner = self.clone();
        self.spawn(ObservationContext::independent().scope(PresenceCheckFeedback::default().scope(async move {
            if lease.protocol() == ConnectionProtocol::PeerReachabilityV2 { runner.serve(&attempt, &lease).await; }
            else { tokio::select! { _ = lease.closed() => {}, _ = attempt.stop.cancelled() => { lease.close(); } } }
            let mut state = runner.evidence.lock().unwrap_or_else(PoisonError::into_inner);
            let id = lease.connection_id();
            if state.connections.remove(&id).is_none() { return; }
            state.verified.remove(&id);
            if state.current(&attempt) && !state.local_failures.contains(&attempt.peer) && !state.connections.values().any(|entry| entry.lease.peer() == attempt.peer && !entry.lease.is_closed()) { runner.publish(&mut state, attempt.peer, ReachabilityState::Offline); }
        })));
    }
    async fn serve(&self, attempt: &Attempt, lease: &ConnectionLease) {
        let mut requests = FuturesUnordered::new();
        loop {
            tokio::select! {
                biased;
                _ = attempt.stop.cancelled() => { lease.close(); break; }
                _ = lease.closed() => break,
                _ = requests.next(), if !requests.is_empty() => {},
                result = lease.accept_bi() => match result {
                    Ok(stream) => { requests.push(self.answer(attempt, lease, stream)); }
                    Err(ConnectivityError::Busy(_)) => continue,
                    Err(_) => break,
                }
            }
        }
        // 停止接新流后等待已经开始的本地工作结算，不能虚报释放它的许可。
        while requests.next().await.is_some() {}
    }
    async fn answer(&self, attempt: &Attempt, lease: &ConnectionLease, mut stream: StreamLease) {
        // 长期监听的 epoch 不变，每个请求分别冻结成功修订。
        let attempt = {
            let state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.current(&attempt) {
                return;
            }
            let mut current = attempt.clone();
            current.success = state.successes.get(&attempt.peer).copied();
            current
        };
        let deadline = Instant::now() + STEP_BUDGET;
        let result = async {
            let nonce = self
                .ports
                .wire
                .read_challenge(&mut *stream.receive, deadline)
                .await?;
            let allowed = self.authorize(&attempt, deadline).await?;
            let current = self
                .evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .current(&attempt);
            if !current || attempt.stop.is_cancelled() || Instant::now() >= deadline {
                return Err(PeerReachabilityTransportError::Stopped);
            }
            self.ports
                .wire
                .write_reply(&mut *stream.send, &nonce, allowed, deadline)
                .await?;
            if !allowed || !current {
                lease.close();
            }
            Ok::<_, PeerReachabilityTransportError>(())
        }
        .await;
        if let Err(source) = result {
            Self::record_failure(&source);
        }
    }
    fn revoke(&self, attempt: &Attempt) {
        let mut state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
        if state.current(&attempt) {
            state.connections.retain(|_, entry| {
                if entry.lease.peer() == attempt.peer {
                    entry.lease.close();
                    false
                } else {
                    true
                }
            });
            self.publish(&mut state, attempt.peer, ReachabilityState::Offline);
            self.ports.environment.forget(&attempt.peer);
        }
    }
}
