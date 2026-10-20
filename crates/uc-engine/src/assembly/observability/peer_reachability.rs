//! 装饰既有完整能力，不查询内部连接、成员标识或协议步骤。
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;
use uc_core::ids::DeviceId;
use uc_core::ports::{
    PeerReachabilityChanged, PeerReachabilityError, PeerReachabilityPort, ReachabilityState,
};
use uc_observability_contract::diagnostics::connectivity::{
    ConfirmationFailure, PresenceCheckObservation, PresenceCheckResult,
};

pub(crate) fn observe_peer_reachability(
    inner: Arc<dyn PeerReachabilityPort>,
) -> Arc<dyn PeerReachabilityPort> {
    Arc::new(ObservedReachability { inner })
}
struct ObservedReachability {
    inner: Arc<dyn PeerReachabilityPort>,
}
fn fallback(result: &Result<ReachabilityState, PeerReachabilityError>) -> PresenceCheckResult {
    match result {
        Ok(ReachabilityState::Online) => PresenceCheckResult::Reachable,
        Err(PeerReachabilityError::NoAddress(_)) => PresenceCheckResult::AddressMissing,
        Ok(ReachabilityState::Offline | ReachabilityState::Unknown) => {
            PresenceCheckResult::Confirmation(ConfirmationFailure::TransportFailed)
        }
        Err(PeerReachabilityError::Internal { .. }) => PresenceCheckResult::Unavailable,
    }
}
#[async_trait]
impl PeerReachabilityPort for ObservedReachability {
    async fn ensure_reachable(
        &self,
        peer: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        let observation = PresenceCheckObservation::begin();
        let result = observation.scope(self.inner.ensure_reachable(peer)).await;
        observation.finish(fallback(&result));
        result
    }
    async fn verify_reachable(
        &self,
        peer: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        let observation = PresenceCheckObservation::begin();
        let result = observation.scope(self.inner.verify_reachable(peer)).await;
        observation.finish(fallback(&result));
        result
    }
    async fn report_communication_failure(&self, peer: &DeviceId) {
        self.inner.report_communication_failure(peer).await;
    }
    async fn forget(&self, peer: &DeviceId) {
        self.inner.forget(peer).await;
    }
    async fn disconnect_all(&self) {
        self.inner.disconnect_all().await;
    }
    async fn activate(&self) {
        self.inner.activate().await;
    }
    async fn current_state(&self, peer: &DeviceId) -> ReachabilityState {
        self.inner.current_state(peer).await
    }
    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.inner.subscribe()
    }
}
