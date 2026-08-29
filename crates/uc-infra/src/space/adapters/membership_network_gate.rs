use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, MembershipNetworkActivityPort,
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding,
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    SpaceAdmissionId, SpaceAdmissionRoute,
};

/// 控制 Space 成员维护是否可以发起新的网络请求。
pub struct MembershipNetworkGate {
    active: AtomicBool,
}

impl MembershipNetworkGate {
    pub fn active() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(true),
        })
    }

    fn permits_network_work(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

impl MembershipNetworkActivityPort for MembershipNetworkGate {
    fn pause_network_work(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn resume_network_work(&self) {
        self.active.store(true, Ordering::Release);
    }
}

pub struct GatedSpaceAdmissionTransport {
    gate: Arc<MembershipNetworkGate>,
    inner: Arc<dyn SpaceAdmissionTransportPort>,
}

impl GatedSpaceAdmissionTransport {
    pub fn new(
        gate: Arc<MembershipNetworkGate>,
        inner: Arc<dyn SpaceAdmissionTransportPort>,
    ) -> Self {
        Self { gate, inner }
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for GatedSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        if !self.gate.permits_network_work() {
            return Err(SpaceAdmissionTransportError::Deferred);
        }
        self.inner
            .establish_initial(admission_id, route, encrypted_password_equivalent)
            .await
    }

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        if !self.gate.permits_network_work() {
            return Err(SpaceAdmissionTransportError::Deferred);
        }
        self.inner
            .resume(admission_id, route, peer_binding, continuation_credential)
            .await
    }
}

pub struct GatedMembershipHistoryExchange {
    gate: Arc<MembershipNetworkGate>,
    inner: Arc<dyn MembershipHistoryExchangePort>,
    restricted: Arc<dyn RestrictedMembershipDeliveryPort>,
}

impl GatedMembershipHistoryExchange {
    pub fn new<T>(gate: Arc<MembershipNetworkGate>, inner: Arc<T>) -> Self
    where
        T: MembershipHistoryExchangePort + RestrictedMembershipDeliveryPort + 'static,
    {
        Self {
            gate,
            inner: inner.clone(),
            restricted: inner,
        }
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for GatedMembershipHistoryExchange {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        if !self.gate.permits_network_work() {
            return Err(MembershipHistoryExchangeError::Offline);
        }
        self.inner
            .exchange_membership_history(recipient, message)
            .await
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for GatedMembershipHistoryExchange {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        if !self.gate.permits_network_work() {
            return Err(RestrictedMembershipDeliveryError::Deferred);
        }
        self.restricted
            .deliver_restricted_membership(peer, delivery)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use uc_core::membership::MembershipHistoryV2Ack;

    use super::*;

    #[derive(Default)]
    struct RecordingHistoryExchange {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl MembershipHistoryExchangePort for RecordingHistoryExchange {
        async fn exchange_membership_history(
            &self,
            _recipient: &DeviceId,
            _message: MembershipHistoryMessage,
        ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Consistent,
            ))
        }
    }

    #[async_trait]
    impl RestrictedMembershipDeliveryPort for RecordingHistoryExchange {
        async fn deliver_restricted_membership(
            &self,
            _peer: &DeviceId,
            _delivery: &RestrictedMembershipDelivery,
        ) -> Result<(), RestrictedMembershipDeliveryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn pause_blocks_new_history_requests_until_resume() {
        let gate = MembershipNetworkGate::active();
        let inner = Arc::new(RecordingHistoryExchange::default());
        let transport = GatedMembershipHistoryExchange::new(gate.clone(), inner.clone());
        let peer = DeviceId::new("peer");
        let message = MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Consistent);

        gate.pause_network_work();
        assert_eq!(
            transport
                .exchange_membership_history(&peer, message.clone())
                .await,
            Err(MembershipHistoryExchangeError::Offline)
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);

        gate.resume_network_work();
        assert!(transport
            .exchange_membership_history(&peer, message)
            .await
            .is_ok());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    }
}
