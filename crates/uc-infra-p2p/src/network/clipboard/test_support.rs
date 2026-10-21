//! 旧回归用例的装配 fixture；生产只使用显式注入 manager 的构造入口。
use super::{ClipboardDispatchAdapter, ClipboardReceiverAdapter};
use crate::network::iroh::admitted_connections::{
    AdmittedConnections, ConnectionRegistration, RevocationEpoch,
};
use crate::network::iroh::connectivity::ConnectivityProfile;
use crate::network::iroh::connectivity::{
    IrohClipboardHandler, IrohConnectionAdmission, IrohConnectivityBackend,
};
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ClipboardReceiverPort, ConnectivityBackend,
    ConnectivityError, ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation,
    PeerConnection, PeerIdentityDirectoryPort, SessionConnectionAdmissionPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{PeerAddressRepositoryPort, PeerReachabilityPort};

/// 仅隔离传输回归；明确不模拟生产成员身份解析。
pub(super) struct EpochOnlyAdmission(pub AdmittedConnections);
struct Attempt {
    registry: AdmittedConnections,
    target: DeviceId,
    epoch: RevocationEpoch,
}
struct Grant {
    registry: AdmittedConnections,
    target: DeviceId,
    epoch: RevocationEpoch,
    _registration: ConnectionRegistration,
}
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.target
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if self.registry.current(&self.target, self.epoch) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
#[async_trait]
impl AdmissionAttempt for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if self.registry.current(&self.target, self.epoch) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let registration = self
            .registry
            .register_managed_since(&self.target, connection, self.epoch)
            .ok_or(ConnectivityError::Denied)?;
        Ok(Box::new(Grant {
            registry: self.registry.clone(),
            target: self.target,
            epoch: self.epoch,
            _registration: registration,
        }))
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for EpochOnlyAdmission {
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            registry: self.0.clone(),
            target,
            epoch: self.0.epoch(&target),
        })
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        _reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        panic!("outbound-only fixture")
    }
}

pub(super) fn dispatch_fixture(
    endpoint: Arc<Endpoint>,
    addresses: Arc<dyn PeerAddressRepositoryPort>,
    reachability: Arc<dyn PeerReachabilityPort>,
    admission: Arc<dyn SessionConnectionAdmissionPort>,
    policy: ConnectivityPolicy,
) -> ClipboardDispatchAdapter {
    ClipboardDispatchAdapter::new(ConnectivityManager::new(
        Arc::new(IrohConnectivityBackend::new(
            endpoint,
            addresses,
            Some(reachability),
            ConnectivityProfile::Clipboard,
        )),
        admission,
        policy,
    ))
}
struct UnusedBackend;
#[async_trait]
impl ConnectivityBackend for UnusedBackend {
    async fn connect(
        &self,
        _: DeviceId,
        _cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        panic!("inbound-only fixture")
    }
}
pub(super) struct ReceiverFixture(Arc<ClipboardReceiverAdapter>);
impl ReceiverFixture {
    pub(super) fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        admission: Arc<dyn PeerAdmissionPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        let admission = Arc::new(IrohConnectionAdmission::new(
            identities,
            admission,
            fingerprint,
            AdmittedConnections::default(),
            ConnectivityProfile::Clipboard,
        ));
        Self(Arc::new(ClipboardReceiverAdapter::new(
            ConnectivityManager::new(
                Arc::new(UnusedBackend),
                admission,
                ConnectivityPolicy::default(),
            ),
        )))
    }
    pub(super) fn subscribe(
        &self,
    ) -> tokio::sync::broadcast::Receiver<uc_application::deps::ClipboardDelivery> {
        self.0.subscribe()
    }
    pub(super) fn handler(&self) -> IrohClipboardHandler {
        IrohClipboardHandler::new(self.0.clone())
    }
}

pub(super) fn dispatch_with_backend(
    backend: Arc<dyn ConnectivityBackend>,
    admission: Arc<dyn SessionConnectionAdmissionPort>,
    policy: ConnectivityPolicy,
) -> ClipboardDispatchAdapter {
    ClipboardDispatchAdapter::new(ConnectivityManager::new(backend, admission, policy))
}
