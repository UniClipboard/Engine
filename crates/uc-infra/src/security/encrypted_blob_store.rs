//! Encrypted blob store decorator.
//!
//! Wraps an inner BlobStorePort and transparently encrypts/decrypts
//! blob data using the session's MasterKey. Uses UCBL binary format
//! with zstd compression for efficient on-disk storage.
//!
//! # Binary Format (V2)
//!
//! ```text
//! [UCBL magic: 4B] [version: 1B] [nonce: 24B] [ciphertext: NB]
//! ```
//!
//! Total header: 29 bytes before ciphertext.
//!
//! # Slice 3 改造
//!
//! 不再走 `BlobCipherPort`——本 decorator 的 wire format（UCBL 二进制）与
//! 4 个剪切板 decorator 用的 JSON `EncryptedBlob` 字节布局不兼容,共享
//! 同一个 port 会破坏既有 `.blob` 文件的可读性（V1 数据兼容 ironclad 不变量）。
//!
//! 改用 `super::v1_aead` 私有 helper 直接调底层 AEAD: 算法行为与历史
//! `EncryptionPort::encrypt_blob` 字节级一致,保证既有 UCBL 文件继续可读。

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info_span, Instrument};

use uc_core::membership::{ContentKeyId, ContentKeyPurpose, GroupEpoch};
use uc_core::{blob::ports::BlobReaderPort, crypto::aad, BlobId, ContentHash};

use super::key_epoch_aad;
use super::session::InMemorySession;
use super::v1_aead;
use crate::blob::{BlobStorePort, StoredPathBlob};

/// Magic bytes identifying a UniClipboard blob file ("UCBL")
const BLOB_MAGIC: [u8; 4] = [0x55, 0x43, 0x42, 0x4C];
const LEGACY_BLOB_FORMAT_VERSION: u8 = 0x01;
const KEYED_BLOB_FORMAT_VERSION: u8 = 0x02;
/// Header size: magic(4) + version(1) + nonce(24) = 29 bytes
const BLOB_HEADER_SIZE: usize = 4 + 1 + 24;
/// zstd compression level (3 = default, good speed/ratio balance)
const ZSTD_LEVEL: i32 = 3;
/// Maximum decompressed size to prevent zip bombs (500 MB)
const MAX_DECOMPRESSED_SIZE: usize = 500 * 1024 * 1024;

/// Serializes a nonce and ciphertext into the UCBL binary format.
#[cfg(test)]
fn serialize_blob(nonce: &[u8; 24], ciphertext: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(BLOB_HEADER_SIZE + ciphertext.len());
    buf.extend_from_slice(&BLOB_MAGIC);
    buf.push(LEGACY_BLOB_FORMAT_VERSION);
    buf.extend_from_slice(nonce);
    buf.extend_from_slice(ciphertext);
    buf
}

/// Parses the UCBL binary format, extracting nonce and ciphertext.
fn serialize_keyed_blob(
    content_key_id: &ContentKeyId,
    epoch: GroupEpoch,
    nonce: &[u8; 24],
    ciphertext: &[u8],
) -> Vec<u8> {
    let key_id = content_key_id.as_str().as_bytes();
    let mut buf = Vec::with_capacity(4 + 1 + 8 + 1 + key_id.len() + 24 + ciphertext.len());
    buf.extend_from_slice(&BLOB_MAGIC);
    buf.push(KEYED_BLOB_FORMAT_VERSION);
    buf.extend_from_slice(&epoch.value().to_le_bytes());
    buf.push(key_id.len() as u8);
    buf.extend_from_slice(key_id);
    buf.extend_from_slice(nonce);
    buf.extend_from_slice(ciphertext);
    buf
}

enum ParsedBlob<'a> {
    Legacy {
        nonce: &'a [u8; 24],
        ciphertext: &'a [u8],
    },
    Keyed {
        content_key_id: ContentKeyId,
        epoch: GroupEpoch,
        nonce: &'a [u8; 24],
        ciphertext: &'a [u8],
    },
}

fn parse_blob(data: &[u8]) -> Result<ParsedBlob<'_>> {
    if data.len() < BLOB_HEADER_SIZE {
        return Err(anyhow::anyhow!(
            "blob file truncated: {} bytes < {} header",
            data.len(),
            BLOB_HEADER_SIZE
        ));
    }
    if data[0..4] != BLOB_MAGIC {
        return Err(anyhow::anyhow!("invalid blob magic bytes"));
    }
    match data[4] {
        LEGACY_BLOB_FORMAT_VERSION => {
            let nonce: &[u8; 24] = data[5..29]
                .try_into()
                .map_err(|_| anyhow::anyhow!("nonce extraction failed"))?;
            Ok(ParsedBlob::Legacy {
                nonce,
                ciphertext: &data[29..],
            })
        }
        KEYED_BLOB_FORMAT_VERSION => {
            const FIXED_PREFIX: usize = 4 + 1 + 8 + 1;
            if data.len() < FIXED_PREFIX + 24 {
                return Err(anyhow::anyhow!("keyed blob header truncated"));
            }
            let epoch = GroupEpoch::new(u64::from_le_bytes(
                data[5..13]
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("epoch extraction failed"))?,
            ));
            let key_id_len = data[13] as usize;
            let nonce_start = FIXED_PREFIX + key_id_len;
            let ciphertext_start = nonce_start + 24;
            if key_id_len == 0 || data.len() < ciphertext_start {
                return Err(anyhow::anyhow!("invalid keyed blob header"));
            }
            let key_id = std::str::from_utf8(&data[FIXED_PREFIX..nonce_start])
                .map_err(|_| anyhow::anyhow!("content key id is not utf-8"))?;
            let content_key_id = ContentKeyId::from_string(key_id)
                .map_err(|_| anyhow::anyhow!("invalid content key id"))?;
            let nonce: &[u8; 24] = data[nonce_start..ciphertext_start]
                .try_into()
                .map_err(|_| anyhow::anyhow!("nonce extraction failed"))?;
            Ok(ParsedBlob::Keyed {
                content_key_id,
                epoch,
                nonce,
                ciphertext: &data[ciphertext_start..],
            })
        }
        version => Err(anyhow::anyhow!(
            "unsupported blob format version: {version}"
        )),
    }
}

/// Decorator that encrypts/decrypts blob data transparently.
///
/// Uses UCBL binary format with zstd compression:
/// - Write: compress -> encrypt -> serialize to binary
/// - Read: parse binary -> decrypt -> decompress
pub struct EncryptedBlobStore {
    inner: Arc<dyn BlobStorePort>,
    session: Arc<InMemorySession>,
}

impl EncryptedBlobStore {
    pub fn new(inner: Arc<dyn BlobStorePort>, session: Arc<InMemorySession>) -> Self {
        Self { inner, session }
    }
}

#[async_trait]
impl BlobStorePort for EncryptedBlobStore {
    async fn put(&self, blob_id: &BlobId, data: &[u8]) -> Result<(PathBuf, Option<i64>)> {
        let plaintext_size = data.len();

        let space_id = self.session.current_space_id()?;
        let resolved = self
            .session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .context("content key is unavailable - cannot encrypt blob")?;

        let compressed =
            zstd::bulk::compress(data, ZSTD_LEVEL).context("failed to compress blob data")?;
        let compressed_size = compressed.len();

        let business_aad = aad::for_blob_v2(blob_id);
        let aad_bytes = key_epoch_aad::bind(
            b"ucbl-v2",
            &space_id,
            resolved.epoch(),
            resolved.content_key_id(),
            ContentKeyPurpose::Content,
            &business_aad,
        );

        // Slice 3 uses the v1_aead helper directly and bypasses the
        // EncryptedBlob wrapper. This store writes the UCBL binary wire format,
        // so it does not need the wrapper's JSON representation.
        let blob = v1_aead::encrypt_blob_xchacha(resolved.key(), &compressed, &aad_bytes)
            .context("failed to encrypt blob data")?;

        let nonce: [u8; 24] = blob
            .nonce
            .as_slice()
            .try_into()
            .context("encrypted blob nonce is not 24 bytes")?;

        let binary_data = serialize_keyed_blob(
            resolved.content_key_id(),
            resolved.epoch(),
            &nonce,
            &blob.ciphertext,
        );
        let on_disk_size = binary_data.len() as i64;

        let (path, _) = self
            .inner
            .put(blob_id, &binary_data)
            .instrument(info_span!("inner_blob_put", blob_id = %blob_id.as_ref()))
            .await?;

        debug!(
            blob_id = %blob_id.as_ref(),
            plaintext_size,
            compressed_size,
            on_disk_size,
            "Wrote V2 blob (compress -> encrypt -> UCBL binary)"
        );

        Ok((path, Some(on_disk_size)))
    }

    async fn put_from_path(
        &self,
        blob_id: &BlobId,
        source_path: &std::path::Path,
    ) -> Result<StoredPathBlob> {
        // AEAD wire format(v1_aead::encrypt_blob_xchacha)是 one-shot,目前不支持流式
        // 加密。这里先把源文件整文件读进内存再走 put() —— 与 capture-side 调用方约定:
        // 加密 store 启用时,path-backed ingest 的"任意大小"语义降级为"内存里能放得下",
        // 流式 AEAD 重构属于独立 phase。
        // No source path in the error context: a clipboard file path is user
        // content (usernames / sensitive filenames) and would leak through the
        // propagated error chain. Correlate by blob_id instead.
        let bytes = tokio::fs::read(source_path).await.with_context(|| {
            format!("failed to read source file for encryption (blob {blob_id})")
        })?;
        // Hash the exact plaintext buffer that gets compressed+encrypted below,
        // in the same read pass — no second read of the source can observe a
        // rewritten file, so the recorded identity matches the stored blob.
        let content_hash = ContentHash::from(blake3::hash(&bytes).as_bytes());
        let size_bytes = bytes.len() as u64;
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
        // Encryption is transparent to deletion: drop the stored bytes via the
        // inner store.
        self.inner.delete(blob_id).await
    }
}

#[async_trait]
impl BlobReaderPort for EncryptedBlobStore {
    async fn get(&self, blob_id: &BlobId) -> Result<Vec<u8>> {
        let binary_data = self
            .inner
            .get(blob_id)
            .instrument(info_span!("inner_blob_get", blob_id = %blob_id.as_ref()))
            .await
            .context("failed to read encrypted blob from storage")?;

        let parsed = parse_blob(&binary_data)?;
        let business_aad = aad::for_blob_v2(blob_id);
        let compressed = match parsed {
            ParsedBlob::Legacy { nonce, ciphertext } => {
                let master_key = self
                    .session
                    .legacy_content_key()
                    .context("encryption session not ready - cannot decrypt blob")?;
                v1_aead::decrypt_blob_xchacha(&master_key, nonce, ciphertext, &business_aad)
                    .context("failed to decrypt legacy blob")?
            }
            ParsedBlob::Keyed {
                content_key_id,
                epoch,
                nonce,
                ciphertext,
            } => {
                let space_id = self.session.current_space_id()?;
                let resolved = self
                    .session
                    .content_key(&space_id, &content_key_id, ContentKeyPurpose::Content)
                    .context("content key is unavailable - cannot decrypt blob")?;
                if resolved.epoch() != epoch {
                    return Err(anyhow::anyhow!("blob key epoch mismatch"));
                }
                let aad_bytes = key_epoch_aad::bind(
                    b"ucbl-v2",
                    &space_id,
                    epoch,
                    &content_key_id,
                    ContentKeyPurpose::Content,
                    &business_aad,
                );
                v1_aead::decrypt_blob_xchacha(resolved.key(), nonce, ciphertext, &aad_bytes)
                    .context("failed to decrypt keyed blob")?
            }
        };

        let plaintext = zstd::bulk::decompress(&compressed, MAX_DECOMPRESSED_SIZE)
            .context("failed to decompress blob data - data may be corrupted")?;

        debug!(
            blob_id = %blob_id.as_ref(),
            on_disk_size = binary_data.len(),
            compressed_size = compressed.len(),
            plaintext_size = plaintext.len(),
            "Read V2 blob (UCBL binary -> decrypt -> decompress)"
        );

        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FilesystemBlobStore;
    use crate::security::secrets::MasterKey;

    fn hash_of(bytes: &[u8]) -> ContentHash {
        ContentHash::from(blake3::hash(bytes).as_bytes())
    }

    fn store_with_key(dir: PathBuf) -> EncryptedBlobStore {
        let inner: Arc<dyn BlobStorePort> = Arc::new(FilesystemBlobStore::new(dir));
        let session = Arc::new(InMemorySession::new());
        let space_id = uc_core::ids::SpaceId::from_str("blob-space");
        session
            .set_master_key_for_space(space_id.clone(), MasterKey::from_bytes(&[7u8; 32]).unwrap());
        let material = session
            .create_migrated_space_material(&space_id, 100)
            .unwrap();
        session.install_space_material(&material).unwrap();
        EncryptedBlobStore::new(inner, session)
    }

    #[tokio::test]
    async fn put_from_path_hashes_plaintext_and_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with_key(tmp.path().join("blobs"));

        let src = tmp.path().join("plain.bin");
        let content = b"encrypted store: recorded identity is the plaintext hash".to_vec();
        tokio::fs::write(&src, &content).await.unwrap();

        let blob_id = BlobId::new();
        let stored = store.put_from_path(&blob_id, &src).await.unwrap();

        assert_eq!(stored.size_bytes, content.len() as u64);
        // Identity is the device-independent *plaintext* hash, never the ciphertext.
        assert_eq!(stored.content_hash, hash_of(&content));
        // Encrypted store tracks the on-disk (ciphertext) size.
        assert!(stored.compressed_size.is_some());

        // Decrypts back to the same plaintext, which hashes to the recorded id.
        let got = BlobReaderPort::get(&store, &blob_id).await.unwrap();
        assert_eq!(got, content);
        assert_eq!(hash_of(&got), stored.content_hash);
    }

    #[tokio::test]
    async fn delete_removes_encrypted_blob_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with_key(tmp.path().join("blobs"));

        let src = tmp.path().join("plain.bin");
        tokio::fs::write(&src, b"bytes").await.unwrap();
        let blob_id = BlobId::new();
        store.put_from_path(&blob_id, &src).await.unwrap();
        assert!(BlobReaderPort::get(&store, &blob_id).await.is_ok());

        store.delete(&blob_id).await.unwrap();
        assert!(BlobReaderPort::get(&store, &blob_id).await.is_err());
        store.delete(&blob_id).await.unwrap();
    }

    #[tokio::test]
    async fn new_blob_header_contains_key_epoch_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_with_key(tmp.path().join("blobs"));
        let blob_id = BlobId::new();

        let (path, _) = store.put(&blob_id, b"secret").await.unwrap();
        let raw = tokio::fs::read(path).await.unwrap();

        assert_eq!(&raw[..4], &BLOB_MAGIC);
        assert_eq!(raw[4], 2);
        assert!(raw
            .windows("legacy-v1".len())
            .all(|window| window != b"legacy-v1"));
        assert_eq!(
            BlobReaderPort::get(&store, &blob_id).await.unwrap(),
            b"secret"
        );
    }

    #[tokio::test]
    async fn legacy_blob_format_remains_readable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("blobs");
        let inner: Arc<dyn BlobStorePort> = Arc::new(FilesystemBlobStore::new(dir));
        let root = MasterKey::from_bytes(&[7u8; 32]).unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = uc_core::ids::SpaceId::from_str("blob-space");
        session.set_master_key_for_space(space_id.clone(), root.clone());
        let material = session
            .create_migrated_space_material(&space_id, 100)
            .unwrap();
        session.install_space_material(&material).unwrap();
        let store = EncryptedBlobStore::new(inner.clone(), session);
        let blob_id = BlobId::new();
        let compressed = zstd::bulk::compress(b"legacy", ZSTD_LEVEL).unwrap();
        let encrypted =
            v1_aead::encrypt_blob_xchacha(&root, &compressed, &aad::for_blob_v2(&blob_id)).unwrap();
        let nonce: [u8; 24] = encrypted.nonce.try_into().unwrap();
        inner
            .put(&blob_id, &serialize_blob(&nonce, &encrypted.ciphertext))
            .await
            .unwrap();

        assert_eq!(
            BlobReaderPort::get(&store, &blob_id).await.unwrap(),
            b"legacy"
        );
    }

    #[tokio::test]
    async fn missing_blob_key_does_not_fall_back_to_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("blobs");
        let writer = store_with_key(dir.clone());
        let blob_id = BlobId::new();
        writer.put(&blob_id, b"secret").await.unwrap();

        let inner: Arc<dyn BlobStorePort> = Arc::new(FilesystemBlobStore::new(dir));
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            uc_core::ids::SpaceId::from_str("blob-space"),
            MasterKey::from_bytes(&[7u8; 32]).unwrap(),
        );
        let reader = EncryptedBlobStore::new(inner, session);

        assert!(BlobReaderPort::get(&reader, &blob_id).await.is_err());
    }
}
