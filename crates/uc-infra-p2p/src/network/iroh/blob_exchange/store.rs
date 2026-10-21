use super::adapter::{IrohBlobExchangeAdapter, RetainedContent};
use async_trait::async_trait;
use bytes::Bytes;
use iroh_blobs::{
    api::blobs::{BlobStatus, ExportMode, ExportOptions},
    Hash, HashAndFormat,
};
use std::path::Path;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobReadStorePort, BlobReceived,
};
use uc_core::ports::blob::BlobDigest;

#[async_trait]
impl BlobReadStorePort for IrohBlobExchangeAdapter {
    async fn protect(&self, digest: BlobDigest) -> Result<Option<BlobReceived>, BlobExchangeError> {
        let hash = Hash::from_bytes(*digest.as_bytes());
        let tag = self
            .store
            .tags()
            .temp_tag(HashAndFormat::raw(hash))
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))?;
        let status = self
            .store
            .blobs()
            .status(hash)
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))?;
        match status {
            BlobStatus::Complete { size } => Ok(Some(BlobReceived {
                digest,
                size,
                protection: Box::new(RetainedContent {
                    _tag: tag,
                    _work: None,
                }),
            })),
            BlobStatus::NotFound | BlobStatus::Partial { .. } => Ok(None),
        }
    }
    async fn read(&self, digest: BlobDigest) -> Result<Bytes, BlobExchangeError> {
        self.store
            .blobs()
            .get_bytes(Hash::from_bytes(*digest.as_bytes()))
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))
    }
    async fn export(&self, digest: BlobDigest, target: &Path) -> Result<u64, BlobExchangeError> {
        self.store
            .blobs()
            .export_with_opts(ExportOptions {
                hash: Hash::from_bytes(*digest.as_bytes()),
                mode: ExportMode::TryReference,
                target: target.to_owned(),
            })
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))
    }
}
