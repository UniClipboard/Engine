use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectivityBackend, ConnectivityError, ConnectivityManager, ConnectivityPolicy,
    PeerConnection, PeerIdentityDirectoryPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_infra_p2p::network::clipboard::ClipboardReceiverAdapter;
use uc_infra_p2p::network::iroh::admitted_connections::AdmittedConnections;
use uc_infra_p2p::network::iroh::connectivity::{ConnectivityProfile, IrohConnectionAdmission};
struct InboundOnly;
#[async_trait]
impl ConnectivityBackend for InboundOnly {
    async fn connect(
        &self,
        _: DeviceId,
        _cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
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
        ConnectivityProfile::Clipboard,
    ));
    Arc::new(ClipboardReceiverAdapter::new(ConnectivityManager::new(
        Arc::new(InboundOnly),
        admission,
        ConnectivityPolicy::default(),
    )))
}
