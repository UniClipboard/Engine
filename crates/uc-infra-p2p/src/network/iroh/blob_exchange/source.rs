use super::super::inbound_peer::PeerIdentityResolver;
use async_trait::async_trait;
use iroh_blobs::{ticket::BlobTicket as NativeBlobTicket, BlobFormat};
use iroh_tickets::Ticket;
use std::sync::Arc;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobSource, BlobSourcePort, PeerIdentityDirectoryPort,
};
use uc_core::ids::DeviceId;
use uc_core::ports::blob::{BlobDigest, BlobTicket};
use uc_core::ports::security::IdentityFingerprintFactoryPort;

/// 只解释既有票据并解析权威设备身份，不建连、不修改地址仓储。
pub struct IrohBlobSourceResolver {
    identities: PeerIdentityResolver,
}
impl IrohBlobSourceResolver {
    pub fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            identities: PeerIdentityResolver::new(identities, fingerprint),
        }
    }
}
#[async_trait]
impl BlobSourcePort for IrohBlobSourceResolver {
    fn inspect(&self, ticket: &BlobTicket) -> Result<BlobSource, BlobExchangeError> {
        let ticket = NativeBlobTicket::decode_bytes(ticket.as_bytes()).map_err(|source| {
            BlobExchangeError::new(BlobExchangeFailure::InvalidRequest, source)
        })?;
        if ticket.format() != BlobFormat::Raw {
            return Err(BlobExchangeError::refused(
                BlobExchangeFailure::InvalidRequest,
            ));
        }
        let address_hint = postcard::to_stdvec(ticket.addr()).map_err(|source| {
            BlobExchangeError::new(BlobExchangeFailure::InvalidRequest, source)
        })?;
        Ok(BlobSource {
            source_key: *ticket.addr().id.as_bytes(),
            digest: BlobDigest::from_bytes(*ticket.hash().as_bytes()),
            address_hint,
        })
    }
    async fn resolve_peer(&self, key: &[u8; 32]) -> Result<DeviceId, BlobExchangeError> {
        self.identities
            .resolve(key)
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Connectivity, source))
    }
}
