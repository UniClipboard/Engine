//! Infra-internal blob repository port.
//!
//! `BlobRepositoryPort` is not a core domain capability: it only exists so that
//! the `BlobWriter` adapter can depend on the SQLite-backed blob row store via
//! an abstraction, keeping the two infra components swappable and testable.
//!
//! Consumers outside the `uc-infra-*` crates should depend on `BlobWriterPort` instead.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uc_core::ContentHash;

use uc_core::BlobId;

use crate::blob::Blob;

/// `delete_if_unreferenced` 在立即事务里的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreferencedBlobDeletion {
    /// 行已删除。`bytes_on_disk` 取压缩后的落盘字节，没有则取明文大小。
    Deleted { bytes_on_disk: u64 },
    /// 行不存在，或三类引用者仍有其一。
    Retained,
}

#[async_trait]
pub trait BlobRepositoryPort: Send + Sync {
    async fn insert_blob(&self, blob: &Blob) -> Result<()>;
    async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>>;

    /// 在同一个 `BEGIN IMMEDIATE` 事务里确认行仍在，并刷新 `last_claimed_at_ms`。
    /// 行已经不在时返回 `None`，调用方必须重新写文件，不能复用已删除的 blob_id。
    async fn claim_existing(
        &self,
        content_hash: &ContentHash,
        claimed_at_ms: i64,
    ) -> Result<Option<Blob>>;

    /// 在 `BEGIN IMMEDIATE` 事务里执行
    /// `DELETE FROM blob WHERE blob_id=? AND NOT EXISTS(三类引用者)`。
    /// 仍被引用时不删除，避免外键 `ON DELETE SET NULL` 悄悄改掉表示。
    async fn delete_if_unreferenced(&self, blob_id: &BlobId) -> Result<UnreferencedBlobDeletion>;
}

#[async_trait]
impl<T: BlobRepositoryPort + ?Sized> BlobRepositoryPort for Arc<T> {
    async fn insert_blob(&self, blob: &Blob) -> Result<()> {
        (**self).insert_blob(blob).await
    }

    async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>> {
        (**self).find_by_hash(content_hash).await
    }

    async fn claim_existing(
        &self,
        content_hash: &ContentHash,
        claimed_at_ms: i64,
    ) -> Result<Option<Blob>> {
        (**self).claim_existing(content_hash, claimed_at_ms).await
    }

    async fn delete_if_unreferenced(&self, blob_id: &BlobId) -> Result<UnreferencedBlobDeletion> {
        (**self).delete_if_unreferenced(blob_id).await
    }
}
