use super::output::PreparedBlobPath;
use std::sync::Arc;

use super::transport::DownloadTask;
use super::transport::{
    BlobDownloadPreparation, BlobDownloadResources, BlobExchangeError, BlobExchangeFailure,
    PreparedDownload,
};
use crate::transfer::receive::DownloadSettlement;
use bytes::Bytes;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use uc_core::ids::EntryId;
use uc_core::ports::blob::{
    BlobDigest, BlobReferenceRepositoryPort, BlobTicket, BlobTransferPort, PlaintextHash, TagReason,
};
use uc_core::ports::security::TransferCipherPort;
use uc_core::ports::ContentHashPort;

pub(crate) struct FetchBlobInput {
    pub(in super::super) settlement: DownloadSettlement,
    pub ticket: BlobTicket,
    pub cancel: CancellationToken,
    pub entry_id: EntryId,
    /// 取得原下载许可后执行的本地准备，返回可选的已校验进度交接。
    pub(in super::super) preparation: BlobDownloadPreparation,
}

#[derive(Debug, Clone)]
pub(crate) struct FetchBlobOutcome {
    pub plaintext: Bytes,
    pub entry_id: EntryId,
    pub plaintext_hash: PlaintextHash,
    pub digest: BlobDigest,
}

/// Streaming variant of [`FetchBlobInput`] — drops `plaintext: Bytes` from
/// the result and writes the blob directly to `target_path`.
///
/// GH#487 Phase 2: receive-side mirror of `PublishBlobInput::Path`. Used by
/// the inbound materializer for free-standing files so a 1 GiB clipboard
/// transfer no longer routes the full plaintext through `Bytes` + a second
/// `tokio::fs::write`.
pub(crate) struct FetchBlobPathInput {
    pub(in super::super) settlement: DownloadSettlement,
    pub ticket: BlobTicket,
    pub cancel: CancellationToken,
    pub entry_id: EntryId,
    pub(super) target_path: PreparedBlobPath,
    pub(in super::super) preparation: BlobDownloadPreparation,
}

#[derive(Debug, Clone)]
pub(crate) struct FetchBlobPathOutcome {
    pub entry_id: EntryId,
    pub plaintext_hash: PlaintextHash,
    pub digest: BlobDigest,
    /// File size on disk after `fetch_to_path` returned. Lets callers (e.g.
    /// the facade emitting the final 100 % progress event) report a real
    /// number even when the upstream `total_bytes` was unknown.
    pub bytes_written: u64,
}

pub(crate) struct FetchBlobUseCase {
    downloads: Arc<BlobDownloadResources>,
    hash: Arc<dyn ContentHashPort>,
    blob_transfer: Arc<dyn BlobTransferPort>,
    blob_reference: Arc<dyn BlobReferenceRepositoryPort>,
    transfer_cipher: Arc<dyn TransferCipherPort>,
}

impl FetchBlobUseCase {
    pub fn new(
        downloads: Arc<BlobDownloadResources>,
        hash: Arc<dyn ContentHashPort>,
        blob_transfer: Arc<dyn BlobTransferPort>,
        blob_reference: Arc<dyn BlobReferenceRepositoryPort>,
        transfer_cipher: Arc<dyn TransferCipherPort>,
    ) -> Self {
        Self {
            downloads,
            hash,
            blob_transfer,
            blob_reference,
            transfer_cipher,
        }
    }

    #[tracing::instrument(name = "usecase.fetch_blob.execute", skip_all)]
    pub(in super::super) fn execute(
        self: &Arc<Self>,
        input: FetchBlobInput,
    ) -> DownloadTask<FetchBlobOutcome> {
        let prepared = match self.downloads.prepare(&input.ticket, &input.cancel) {
            Ok(prepared) => prepared,
            Err(error) => return DownloadTask::rejected(error, input.cancel),
        };
        let owner = self.clone();
        self.downloads
            .transport
            .supervise(input.cancel.clone(), async move {
                owner
                    .execute_inner(input, prepared)
                    .await
                    .map_err(fetch_error)
            })
    }
    async fn execute_inner(
        &self,
        input: FetchBlobInput,
        prepared: PreparedDownload,
    ) -> Result<FetchBlobOutcome, FetchBlobError> {
        let received = self
            .downloads
            .obtain(
                prepared,
                &input.cancel,
                input.preparation,
                input.settlement.clone(),
            )
            .await
            .map_err(|source| FetchBlobError::Transfer(source.into()))?;
        let digest = received.digest;
        if input.cancel.is_cancelled() {
            return Err(FetchBlobError::Cancelled(BlobExchangeFailure::Cancelled));
        }
        let encrypted_bytes = self
            .downloads
            .store
            .read(digest)
            .await
            .map_err(|source| FetchBlobError::Transfer(source.into()))?;
        if input.cancel.is_cancelled() {
            return Err(FetchBlobError::Cancelled(BlobExchangeFailure::Cancelled));
        }
        let plaintext_bytes = Bytes::from(
            self.transfer_cipher
                .decrypt(&encrypted_bytes)
                .await
                .map_err(|e| FetchBlobError::Cipher(anyhow::Error::from(e)))?,
        );
        let plaintext_hash = PlaintextHash::from_bytes(
            self.hash
                .hash_bytes(&plaintext_bytes)
                .map_err(|e| FetchBlobError::Hash(anyhow::Error::from(e)))?
                .bytes,
        );

        received
            .request
            .validate(Instant::now() + Duration::from_secs(60), &input.cancel)
            .await
            .map_err(|source| FetchBlobError::Transfer(source.into()))?;
        self.blob_reference
            .save(plaintext_hash, digest)
            .await
            .map_err(|e| FetchBlobError::Reference(anyhow::Error::from(e)))?;
        self.blob_transfer
            .tag(&digest, TagReason::ClipboardEntry(input.entry_id.clone()))
            .await
            .map_err(|e| FetchBlobError::Transfer(anyhow::Error::from(e)))?;

        Ok(FetchBlobOutcome {
            plaintext: plaintext_bytes,
            entry_id: input.entry_id,
            plaintext_hash,
            digest,
        })
    }

    /// Raw 文件经 SDK 校验后流式导出；内容保护及下载许可继续覆盖引用和标签结算。
    #[tracing::instrument(name = "usecase.fetch_blob.execute_to_path", skip_all)]
    pub(in super::super) fn execute_to_path(
        self: &Arc<Self>,
        input: FetchBlobPathInput,
    ) -> DownloadTask<FetchBlobPathOutcome> {
        let prepared = match self.downloads.prepare(&input.ticket, &input.cancel) {
            Ok(prepared) => prepared,
            Err(error) => return DownloadTask::rejected(error, input.cancel),
        };
        let owner = self.clone();
        self.downloads
            .transport
            .supervise(input.cancel.clone(), async move {
                owner
                    .execute_path_inner(input, prepared)
                    .await
                    .map_err(fetch_error)
            })
    }
    async fn execute_path_inner(
        &self,
        input: FetchBlobPathInput,
        prepared: PreparedDownload,
    ) -> Result<FetchBlobPathOutcome, FetchBlobError> {
        let received = self
            .downloads
            .obtain(
                prepared,
                &input.cancel,
                input.preparation,
                input.settlement.clone(),
            )
            .await
            .map_err(|source| FetchBlobError::Transfer(source.into()))?;
        let digest = received.digest;
        if input.cancel.is_cancelled() {
            return Err(FetchBlobError::Cancelled(BlobExchangeFailure::Cancelled));
        }
        let bytes_written = self
            .downloads
            .store
            .export(
                digest,
                input
                    .target_path
                    .path()
                    .map_err(|source| FetchBlobError::Transfer(source.into()))?,
            )
            .await
            .map_err(|source| FetchBlobError::Transfer(source.into()))?;
        if input.cancel.is_cancelled() {
            return Err(FetchBlobError::Cancelled(BlobExchangeFailure::Cancelled));
        }

        // File blobs are content-addressed by blake3 of the raw plaintext;
        // since they're stored without encryption the adapter's digest IS
        // the plaintext hash (mirrors `PublishBlobUseCase::execute_path`).
        let plaintext_hash = PlaintextHash::from_bytes(*digest.as_bytes());

        received
            .request
            .validate(Instant::now() + Duration::from_secs(60), &input.cancel)
            .await
            .map_err(|source| FetchBlobError::Transfer(source.into()))?;
        self.blob_reference
            .save(plaintext_hash, digest)
            .await
            .map_err(|e| FetchBlobError::Reference(anyhow::Error::from(e)))?;
        self.blob_transfer
            .tag(&digest, TagReason::ClipboardEntry(input.entry_id.clone()))
            .await
            .map_err(|e| FetchBlobError::Transfer(anyhow::Error::from(e)))?;

        Ok(FetchBlobPathOutcome {
            entry_id: input.entry_id,
            plaintext_hash,
            digest,
            bytes_written,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchBlobError {
    #[error("blob fetch cancelled")]
    Cancelled(#[source] BlobExchangeFailure),
    #[error("hash failed")]
    Hash(#[source] anyhow::Error),
    #[error("blob transfer failed")]
    Transfer(#[source] anyhow::Error),
    #[error("blob reference failed")]
    Reference(#[source] anyhow::Error),
    #[error("blob payload decryption failed")]
    Cipher(#[source] anyhow::Error),
}

fn fetch_error(source: FetchBlobError) -> BlobExchangeError {
    let kind = match &source {
        FetchBlobError::Cancelled(_) => BlobExchangeFailure::Cancelled,
        FetchBlobError::Cipher(_) | FetchBlobError::Hash(_) => BlobExchangeFailure::Integrity,
        FetchBlobError::Reference(_) => BlobExchangeFailure::Store,
        FetchBlobError::Transfer(error) => error
            .chain()
            .find_map(|source| {
                source
                    .downcast_ref::<BlobExchangeError>()
                    .map(|error| error.kind)
            })
            .unwrap_or(BlobExchangeFailure::Store),
    };
    BlobExchangeError::new(kind, source)
}
