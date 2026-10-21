//! 获取流程测试复用本地能力，网络／授权边界另由真实 Uni 测试覆盖。
use super::transport::{
    BlobExchangeError, BlobExchangeFailure, BlobExchangePort, BlobReadStorePort, BlobReceived,
    BlobRetainedContent, BlobSource, BlobSourcePort, BlobStoreLifecyclePort, BlobValidatedProgress,
};
use crate::connectivity::{
    AdmissionAttempt, AdmissionGrant, ConnectivityBackend, ConnectivityError,
    InboundIdentityReservation, PeerConnection, SessionConnectionAdmissionPort, StreamLease,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::path::Path;
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::blob::{BlobDigest, BlobTicket, BlobTransferPort};

pub(super) struct FakeLocalStore(pub Arc<dyn BlobTransferPort>);
struct Protection;
impl BlobRetainedContent for Protection {}
#[async_trait]
impl BlobReadStorePort for FakeLocalStore {
    async fn protect(&self, digest: BlobDigest) -> Result<Option<BlobReceived>, BlobExchangeError> {
        Ok(Some(BlobReceived {
            digest,
            size: 0,
            protection: Box::new(Protection),
        }))
    }
    async fn read(&self, digest: BlobDigest) -> Result<Bytes, BlobExchangeError> {
        self.0
            .fetch(&BlobTicket::from_bytes(digest.as_bytes().to_vec()), None)
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))
    }
    async fn export(&self, digest: BlobDigest, target: &Path) -> Result<u64, BlobExchangeError> {
        self.0
            .fetch_to_path(
                &BlobTicket::from_bytes(digest.as_bytes().to_vec()),
                target,
                None,
            )
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))?;
        Ok(tokio::fs::metadata(target)
            .await
            .expect("export metadata")
            .len())
    }
}
#[async_trait]
impl BlobSourcePort for FakeLocalStore {
    fn inspect(&self, ticket: &BlobTicket) -> Result<BlobSource, BlobExchangeError> {
        Ok(BlobSource {
            source_key: [0; 32],
            digest: self.0.digest_of(ticket).map_err(|source| {
                BlobExchangeError::new(BlobExchangeFailure::InvalidRequest, source)
            })?,
            address_hint: Vec::new(),
        })
    }
    async fn resolve_peer(&self, _: &[u8; 32]) -> Result<DeviceId, BlobExchangeError> {
        Ok(DeviceId::new("peer"))
    }
}
pub(super) struct RejectUnexpectedNetwork;
#[async_trait]
impl ConnectivityBackend for RejectUnexpectedNetwork {
    async fn connect(
        &self,
        _: DeviceId,
        _: Option<Vec<u8>>,
        _: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        panic!("local fixture cannot dial")
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for RejectUnexpectedNetwork {
    fn begin(&self, _: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(RejectUnexpectedNetwork)
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn PeerConnection>,
        _: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        panic!("local fixture cannot serve")
    }
}
#[async_trait]
impl AdmissionAttempt for RejectUnexpectedNetwork {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        _: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        panic!("local fixture cannot connect")
    }
}
#[async_trait]
impl BlobExchangePort for RejectUnexpectedNetwork {
    async fn receive(
        &self,
        _: StreamLease,
        _: BlobDigest,
        _: CancellationToken,
        _: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        panic!("local fixture cannot receive")
    }
    async fn provide(
        &self,
        _: StreamLease,
        _: Instant,
        _: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        panic!("local fixture cannot provide")
    }
}
#[async_trait]
impl BlobStoreLifecyclePort for RejectUnexpectedNetwork {
    async fn shutdown(&self) -> Result<(), BlobExchangeError> {
        Ok(())
    }
}
