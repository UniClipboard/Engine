use async_trait::async_trait;
use std::sync::Arc;
use uc_application::deps::{
    ConnectivityBackend, ConnectivityError, ConnectivityManager, ConnectivityPolicy,
    PeerConnection, PeerIdentityDirectoryPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_infra_p2p::network::clipboard::ClipboardReceiverAdapter;
use uc_infra_p2p::network::iroh::admitted_connections::AdmittedConnections;
use uc_infra_p2p::network::iroh::connectivity::IrohConnectionAdmission;
struct InboundOnly;
#[async_trait]
impl ConnectivityBackend for InboundOnly {
    async fn connect(&self, _: DeviceId) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        panic!("inbound-only test")
    }
}
pub fn receiver(
    identities: Arc<dyn PeerIdentityDirectoryPort>,
    admission: Arc<dyn PeerAdmissionPort>,
    fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
) -> Arc<ClipboardReceiverAdapter> {
    let admission = Arc::new(IrohConnectionAdmission::new(
        identities,
        admission,
        fingerprint,
        AdmittedConnections::default(),
    ));
    Arc::new(ClipboardReceiverAdapter::new(ConnectivityManager::new(
        Arc::new(InboundOnly),
        admission,
        ConnectivityPolicy::default(),
    )))
}
