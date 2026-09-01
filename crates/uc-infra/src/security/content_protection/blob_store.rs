use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, info_span, Instrument};
use uc_core::blob::ports::BlobReaderPort;
use uc_core::crypto::aad;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::{BlobId, ContentHash};

use super::ContentProtection;
use crate::blob::{BlobStorePort, StoredPathBlob};

const BLOB_MAGIC: [u8; 4] = *b"UCBL";
const BLOB_FORMAT_VERSION_V3: u8 = 3;
const BLOB_HEADER_BYTES: usize = BLOB_MAGIC.len() + 1;
const ZSTD_LEVEL: i32 = 3;
const MAX_DECOMPRESSED_SIZE: usize = 500 * 1024 * 1024;

/// 只读写 UCBL V3 的 profile blob store。
///
/// 压缩和 UCBL 外层 framing 属于本类型；保护上下文、key resolution 与 AEAD
/// envelope 完整委托给 `ContentProtection`，不在此复制密钥选择逻辑。
pub struct V3EncryptedBlobStore {
    inner: Arc<dyn BlobStorePort>,
    protection: Arc<ContentProtection>,
}

impl V3EncryptedBlobStore {
    pub fn new(inner: Arc<dyn BlobStorePort>, protection: Arc<ContentProtection>) -> Self {
        Self { inner, protection }
    }
}

#[async_trait]
impl BlobStorePort for V3EncryptedBlobStore {
    async fn put(&self, blob_id: &BlobId, data: &[u8]) -> Result<(PathBuf, Option<i64>)> {
        let plaintext_size = data.len();
        let compressed =
            zstd::bulk::compress(data, ZSTD_LEVEL).context("compress V3 profile blob")?;
        let compressed_size = compressed.len();
        let ciphertext = self
            .protection
            .seal_for_active(
                &Plaintext::new(compressed),
                &Aad::new(aad::for_blob_v3(blob_id)),
            )
            .await
            .context("protect V3 profile blob")?;
        let binary_data = encode_blob(ciphertext.as_bytes())?;
        let on_disk_size = i64::try_from(binary_data.len()).context("V3 blob size overflow")?;
        let (path, _) = self
            .inner
            .put(blob_id, &binary_data)
            .instrument(info_span!("v3_inner_blob_put"))
            .await
            .context("persist V3 profile blob")?;
        debug!(
            plaintext_size,
            compressed_size, on_disk_size, "Wrote V3 profile blob"
        );
        Ok((path, Some(on_disk_size)))
    }

    async fn put_from_path(
        &self,
        blob_id: &BlobId,
        source_path: &std::path::Path,
    ) -> Result<StoredPathBlob> {
        let bytes = tokio::fs::read(source_path)
            .await
            .context("read source file for V3 profile blob")?;
        let content_hash = ContentHash::from(blake3::hash(&bytes).as_bytes());
        let size_bytes = u64::try_from(bytes.len()).context("V3 source blob size overflow")?;
        let (storage_path, compressed_size) = self.put(blob_id, &bytes).await?;
        Ok(StoredPathBlob {
            storage_path,
            content_hash,
            size_bytes,
            compressed_size,
        })
    }

    async fn get(&self, blob_id: &BlobId) -> Result<Vec<u8>> {
        <Self as BlobReaderPort>::get(self, blob_id).await
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<()> {
        self.inner.delete(blob_id).await
    }
}

#[async_trait]
impl BlobReaderPort for V3EncryptedBlobStore {
    async fn get(&self, blob_id: &BlobId) -> Result<Vec<u8>> {
        let binary_data = self
            .inner
            .get(blob_id)
            .instrument(info_span!("v3_inner_blob_get"))
            .await
            .context("read V3 profile blob")?;
        let ciphertext = Ciphertext::new(parse_blob(&binary_data)?.to_vec());
        let compressed = self
            .protection
            .open(&ciphertext, &Aad::new(aad::for_blob_v3(blob_id)))
            .await
            .context("open V3 profile blob")?;
        let plaintext = zstd::bulk::decompress(compressed.as_bytes(), MAX_DECOMPRESSED_SIZE)
            .context("decompress V3 profile blob")?;
        debug!(
            on_disk_size = binary_data.len(),
            compressed_size = compressed.len(),
            plaintext_size = plaintext.len(),
            "Read V3 profile blob"
        );
        Ok(plaintext)
    }
}

fn encode_blob(ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.is_empty() {
        return Err(anyhow::anyhow!("V3 content envelope is empty"));
    }
    let capacity = BLOB_HEADER_BYTES
        .checked_add(ciphertext.len())
        .context("V3 UCBL length overflow")?;
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(&BLOB_MAGIC);
    output.push(BLOB_FORMAT_VERSION_V3);
    output.extend_from_slice(ciphertext);
    Ok(output)
}

fn parse_blob(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() <= BLOB_HEADER_BYTES {
        return Err(anyhow::anyhow!("V3 UCBL header is truncated"));
    }
    if bytes[..BLOB_MAGIC.len()] != BLOB_MAGIC {
        return Err(anyhow::anyhow!("V3 UCBL magic is invalid"));
    }
    if bytes[BLOB_MAGIC.len()] != BLOB_FORMAT_VERSION_V3 {
        return Err(anyhow::anyhow!("unsupported UCBL profile blob version"));
    }
    Ok(&bytes[BLOB_HEADER_BYTES..])
}
