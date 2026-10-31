use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug_span, Instrument};
use uc_core::blob::ports::{BlobContentIngestPort, BlobWriterPort, IngestedBlob};
use uc_core::ports::ClockPort;
use uc_core::BlobId;
use uc_core::ContentHash;
use uc_observability_contract::{
    error_source::io_error_kind,
    log_fields::{log_id, log_vocab},
    uc_debug, uc_warn,
};

use super::lease::{BlobLeaseGuard, BlobLeaseRegistry};
use crate::blob::hashing::stream_hash_file;
use crate::blob::{Blob, BlobRepositoryPort, BlobStorageLocator, BlobStorePort, StoredPathBlob};

pub struct BlobWriter<B, BR, C>
where
    B: BlobStorePort,
    BR: BlobRepositoryPort,
    C: ClockPort,
{
    blob_store: B,
    blob_repo: BR,
    clock: C,
    leases: Arc<BlobLeaseRegistry>,
}

impl<B, BR, C> BlobWriter<B, BR, C>
where
    B: BlobStorePort,
    BR: BlobRepositoryPort,
    C: ClockPort,
{
    pub fn new(blob_store: B, blob_repo: BR, clock: C, leases: Arc<BlobLeaseRegistry>) -> Self {
        BlobWriter {
            blob_store,
            blob_repo,
            clock,
            leases,
        }
    }

    /// Stream a path-backed file into blob storage, deduplicating by content
    /// hash, and surface the storage handle together with the content hash and
    /// byte size computed during the single storing pass.
    ///
    /// Shared core for both `BlobWriterPort::write_path_if_absent` (which
    /// discards everything but the `BlobId`) and `BlobContentIngestPort::ingest_path`.
    ///
    /// The content hash is derived by the store from the exact bytes it persists
    /// (a single pass), so the recorded `content_hash`/`size_bytes` can never
    /// diverge from the stored blob — there is no separate hash pass that a
    /// concurrent source rewrite could race against. Deduplication therefore runs
    /// *after* the write, against the authoritative hash; on a dedup hit the
    /// freshly written blob is dropped and the existing record is reused.
    async fn ingest_path_inner(&self, source_path: &Path) -> Result<IngestedBlob> {
        // No source_path field: a clipboard file path is user content (it can
        // carry usernames / sensitive filenames). The inner logs identify the
        // op by blob_id and size instead.
        let span = debug_span!("infra.blob.ingest_path");
        let source = source_path.to_path_buf();

        async move {
            // 1. 落盘:store 在写入的同一遍里算出权威 content_hash + size,二者与实际
            //    持有的字节同源(hardlink 时哈希 dest,copy/加密时哈希落盘字节流)。
            let blob_id = BlobId::new();
            // put 前登记。守卫在失败或新 id 返回时释放；去重命中的已有 id 才 persist。
            let fresh_lease = self.leases.acquire_guard(&blob_id);
            let StoredPathBlob {
                storage_path,
                content_hash,
                size_bytes,
                compressed_size,
            } = match self.blob_store.put_from_path(&blob_id, &source).await {
                Ok(stored) => stored,
                Err(err) => {
                    // A partial write (mid-copy read error, post-link hash
                    // failure, …) may have left a file under this blob_id. Clean
                    // it up so a failed ingest never leaks an orphan blob.
                    self.discard_blob(&blob_id, "put_from_path failed").await;
                    drop(fresh_lease);
                    return Err(err);
                }
            };
            uc_debug!(
                file_size = size_bytes,
                "Persisted path-backed blob; recording by authoritative content hash"
            );

            // 2. 按权威 hash 去重。命中时先登记租约，再在 IMMEDIATE 事务里确认行仍在
            //    并刷新 last_claimed_at。行已经不在就不能复用那个 blob_id，改为插入刚写的文件。
            let created_at_ms = self.clock.now_ms();
            let looked_up = match self.blob_repo.find_by_hash(&content_hash).await {
                Ok(found) => found,
                Err(err) => {
                    self.discard_blob(&blob_id, "dedup lookup failed").await;
                    drop(fresh_lease);
                    return Err(err);
                }
            };
            let looked_up_lease = looked_up
                .as_ref()
                .map(|existing| self.leases.acquire_guard(&existing.blob_id));
            match self
                .blob_repo
                .claim_existing(&content_hash, created_at_ms)
                .await
            {
                Ok(Some(existing)) => {
                    persist_claimed_lease(looked_up_lease, &self.leases, &existing.blob_id);
                    uc_debug!(
                        blob_id = log_id(&existing.blob_id),
                        "Path ingest: dedup hit, dropping freshly written blob and reusing existing"
                    );
                    // 新文件即使是硬链接，也只 unlink 这个新 blob_id 的名字。
                    // 源文件和其他链接还在时，inode 不会被删掉。
                    self.discard_blob(&blob_id, "dedup hit").await;
                    drop(fresh_lease);
                    return Ok(IngestedBlob {
                        blob_id: existing.blob_id,
                        content_hash,
                        size_bytes,
                    });
                }
                Ok(None) => {
                    // 查找命中后行已被删。释放那个旧 id 的租约，下面插入本次新文件。
                    drop(looked_up_lease);
                }
                Err(err) => {
                    drop(looked_up_lease);
                    self.discard_blob(&blob_id, "dedup lookup failed").await;
                    drop(fresh_lease);
                    return Err(err);
                }
            }

            let blob_storage_locator = BlobStorageLocator::new_local_fs(storage_path);
            let record = Blob::new(
                blob_id.clone(),
                blob_storage_locator,
                size_bytes as i64,
                content_hash.clone(),
                created_at_ms,
                created_at_ms,
                compressed_size,
            );

            if let Err(insert_err) = self.blob_repo.insert_blob(&record).await {
                // Insert most likely lost the content_hash UNIQUE race with a
                // concurrent ingest; reuse the winner's record if it is present.
                match self
                    .blob_repo
                    .claim_existing(&content_hash, created_at_ms)
                    .await
                {
                    Ok(Some(existing)) => {
                        self.leases.acquire_guard(&existing.blob_id).persist();
                        uc_debug!(
                            error_kind = "insert_race",
                            io_error_kind = io_error_kind(insert_err.as_ref()),
                            "Path ingest insert raced with existing blob; dropping freshly written blob and returning existing record",
                        );
                        self.discard_blob(&blob_id, "insert race").await;
                        drop(fresh_lease);
                        return Ok(IngestedBlob {
                            blob_id: existing.blob_id,
                            content_hash,
                            size_bytes,
                        });
                    }
                    Ok(None) => {}
                    Err(err) => {
                        self.discard_blob(&blob_id, "insert failed").await;
                        drop(fresh_lease);
                        return Err(err);
                    }
                }
                // 竞态赢家也已经不在。不能复用已删除的 id；刚写入的文件属于本次新 id，再插一次。
                if let Err(retry_err) = self.blob_repo.insert_blob(&record).await {
                    self.discard_blob(&blob_id, "insert failed").await;
                    drop(fresh_lease);
                    return Err(retry_err);
                }
            }
            // 新 id 还没有任何条目引用。返回前释放守卫，引用落库后的删除
            // 只受认领宽限约束，不由进程内租约挡住。该宽限需实测确认。
            drop(fresh_lease);
            Ok(IngestedBlob {
                blob_id,
                content_hash,
                size_bytes,
            })
        }
        .instrument(span)
        .await
    }

    /// Remove a blob that was written but will not be referenced by any record
    /// (dedup hit, insert race, or a failed write that left a partial file). A
    /// failed cleanup is logged but not propagated: the redundant blob is an
    /// orphan, not a correctness fault for the ingest the caller asked for.
    async fn discard_blob(&self, blob_id: &BlobId, reason: &str) {
        if let Err(err) = self.blob_store.delete(blob_id).await {
            uc_warn!(
                error_kind = "redundant_blob_remove",
                io_error_kind = io_error_kind(err.as_ref()),
                blob_id = log_id(&blob_id),
                reason = log_vocab(&reason),
                "Failed to remove redundant blob during path ingest cleanup"
            );
        }
    }
}

#[async_trait]
impl<B, BR, C> BlobWriterPort for BlobWriter<B, BR, C>
where
    B: BlobStorePort,
    BR: BlobRepositoryPort,
    C: ClockPort,
{
    async fn write_if_absent(
        &self,
        content_id: &ContentHash,
        plaintext_bytes: &[u8],
    ) -> Result<BlobId> {
        let span = debug_span!(
            "infra.blob.write_if_absent",
            size_bytes = plaintext_bytes.len(),
            content_hash = %content_id,
        );
        async {
            let claimed_at_ms = self.clock.now_ms();
            if let Some(existing) = self.blob_repo.find_by_hash(content_id).await? {
                let looked_up_lease = self.leases.acquire_guard(&existing.blob_id);
                match self
                    .blob_repo
                    .claim_existing(content_id, claimed_at_ms)
                    .await
                {
                    Ok(Some(claimed)) => {
                        persist_claimed_lease(
                            Some(looked_up_lease),
                            &self.leases,
                            &claimed.blob_id,
                        );
                        return Ok(claimed.blob_id);
                    }
                    Ok(None) => {
                        // 行在命中后被删除。不能复用旧 blob_id，改为重新写文件。
                        drop(looked_up_lease);
                    }
                    Err(err) => {
                        drop(looked_up_lease);
                        return Err(err);
                    }
                }
            }

            let blob_id = BlobId::new();
            let fresh_lease = self.leases.acquire_guard(&blob_id);

            // Encryption is handled by the injected BlobStorePort decorator (if any).
            let (storage_path, compressed_size) =
                match self.blob_store.put(&blob_id, plaintext_bytes).await {
                    Ok(stored) => stored,
                    Err(err) => {
                        self.discard_blob(&blob_id, "put failed").await;
                        drop(fresh_lease);
                        return Err(err);
                    }
                };

            let record = Blob::new(
                blob_id.clone(),
                BlobStorageLocator::new_local_fs(storage_path),
                plaintext_bytes.len() as i64,
                content_id.clone(),
                claimed_at_ms,
                claimed_at_ms,
                compressed_size,
            );

            if let Err(err) = self.blob_repo.insert_blob(&record).await {
                match self
                    .blob_repo
                    .claim_existing(content_id, claimed_at_ms)
                    .await
                {
                    Ok(Some(existing)) => {
                        self.leases.acquire_guard(&existing.blob_id).persist();
                        uc_debug!(
                            error_kind = "insert_race",
                            io_error_kind = io_error_kind(err.as_ref()),
                            "Insert raced with existing blob; returning existing record",
                        );
                        // put 按新 blob_id 新建文件，不覆盖已有 blob 的路径。
                        // 硬链接场景下 unlink 新名字也不会删除仍被其他链接持有的 inode。
                        self.discard_blob(&blob_id, "insert race").await;
                        drop(fresh_lease);
                        return Ok(existing.blob_id);
                    }
                    Ok(None) => {}
                    Err(claim_err) => {
                        self.discard_blob(&blob_id, "insert failed").await;
                        drop(fresh_lease);
                        return Err(claim_err);
                    }
                }
                self.discard_blob(&blob_id, "insert failed").await;
                drop(fresh_lease);
                return Err(err);
            }
            // 新 id 尚未被表示引用。返回前释放守卫。去重命中的已有 id
            // 由调用方在引用提交后释放。认领宽限需实测确认。
            drop(fresh_lease);
            Ok(blob_id)
        }
        .instrument(span)
        .await
    }

    async fn write_path_if_absent(&self, source_path: &Path) -> Result<BlobId> {
        Ok(self.ingest_path_inner(source_path).await?.blob_id)
    }
}

/// 认领到的 id 与查找时登记的 id 相同时只保留那一次计数；不一致则先丢掉查找租约。
fn persist_claimed_lease(
    looked_up: Option<BlobLeaseGuard>,
    leases: &Arc<BlobLeaseRegistry>,
    claimed_id: &BlobId,
) {
    match looked_up {
        Some(guard) if guard.blob_id() == claimed_id => guard.persist(),
        looked_up => {
            drop(looked_up);
            leases.acquire_guard(claimed_id).persist();
        }
    }
}

#[async_trait]
impl<B, BR, C> BlobContentIngestPort for BlobWriter<B, BR, C>
where
    B: BlobStorePort,
    BR: BlobRepositoryPort,
    C: ClockPort,
{
    async fn ingest_path(&self, source_path: &Path) -> Result<IngestedBlob> {
        self.ingest_path_inner(source_path).await
    }

    async fn hash_path(&self, source_path: &Path) -> Result<ContentHash> {
        let span = debug_span!("infra.blob.hash_path");
        let source = source_path.to_path_buf();
        async move {
            // Stream the file to compute its ContentHash without loading it into
            // memory or writing any blob — identity only, no materialization.
            let (content_id, file_size) =
                tokio::task::spawn_blocking(move || stream_hash_file(&source))
                    .await
                    .context("hash join failed")??;
            uc_debug!(
                file_size = file_size,
                "Computed content hash for path (identity only, no materialization)"
            );
            Ok(content_id)
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{FilesystemBlobStore, UnreferencedBlobDeletion};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    struct FixedClock(i64);
    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    /// In-memory blob repository that enforces the same `content_hash UNIQUE`
    /// constraint the SQLite-backed repository relies on, so the dedup/insert-race
    /// paths exercise realistic behaviour.
    #[derive(Default)]
    struct InMemoryBlobRepo {
        rows: Mutex<Vec<Blob>>,
    }

    #[async_trait]
    impl BlobRepositoryPort for InMemoryBlobRepo {
        async fn insert_blob(&self, blob: &Blob) -> Result<()> {
            let mut rows = self.rows.lock().unwrap();
            if rows.iter().any(|b| b.content_hash == blob.content_hash) {
                anyhow::bail!("UNIQUE constraint failed: blob.content_hash");
            }
            rows.push(blob.clone());
            Ok(())
        }

        async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>> {
            let rows = self.rows.lock().unwrap();
            Ok(rows
                .iter()
                .find(|b| &b.content_hash == content_hash)
                .cloned())
        }

        async fn claim_existing(
            &self,
            content_hash: &ContentHash,
            claimed_at_ms: i64,
        ) -> Result<Option<Blob>> {
            let mut rows = self.rows.lock().unwrap();
            let Some(row) = rows.iter_mut().find(|b| &b.content_hash == content_hash) else {
                return Ok(None);
            };
            row.last_claimed_at_ms = claimed_at_ms;
            Ok(Some(row.clone()))
        }

        async fn delete_if_unreferenced(
            &self,
            _blob_id: &BlobId,
            _claimed_before_ms: i64,
        ) -> Result<UnreferencedBlobDeletion> {
            Ok(UnreferencedBlobDeletion::Retained)
        }
    }

    /// 第一次 find 能看到行，紧接着的 claim 发现行已被删。
    struct VanishOnClaimRepo {
        inner: InMemoryBlobRepo,
        vanish: AtomicBool,
    }

    #[async_trait]
    impl BlobRepositoryPort for VanishOnClaimRepo {
        async fn insert_blob(&self, blob: &Blob) -> Result<()> {
            self.inner.insert_blob(blob).await
        }

        async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>> {
            self.inner.find_by_hash(content_hash).await
        }

        async fn claim_existing(
            &self,
            content_hash: &ContentHash,
            claimed_at_ms: i64,
        ) -> Result<Option<Blob>> {
            if self.vanish.swap(false, Ordering::SeqCst) {
                let mut rows = self.inner.rows.lock().unwrap();
                rows.retain(|row| &row.content_hash != content_hash);
                return Ok(None);
            }
            self.inner.claim_existing(content_hash, claimed_at_ms).await
        }

        async fn delete_if_unreferenced(
            &self,
            blob_id: &BlobId,
            claimed_before_ms: i64,
        ) -> Result<UnreferencedBlobDeletion> {
            self.inner
                .delete_if_unreferenced(blob_id, claimed_before_ms)
                .await
        }
    }

    fn hash_of(bytes: &[u8]) -> ContentHash {
        ContentHash::from(blake3::hash(bytes).as_bytes())
    }

    fn count_blob_files(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|rd| rd.count()).unwrap_or(0)
    }

    #[tokio::test]
    async fn ingest_records_hash_matching_stored_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(FilesystemBlobStore::new(tmp.path().join("blobs")));
        let repo = Arc::new(InMemoryBlobRepo::default());
        let writer = BlobWriter::new(
            store.clone(),
            repo,
            FixedClock(123),
            Arc::new(BlobLeaseRegistry::isolated()),
        );

        let src = tmp.path().join("a.bin");
        let content = b"writer ingest: identity matches stored bytes".to_vec();
        tokio::fs::write(&src, &content).await.unwrap();

        let ingested = writer.ingest_path(&src).await.unwrap();
        assert_eq!(ingested.content_hash, hash_of(&content));
        assert_eq!(ingested.size_bytes, content.len() as u64);

        let stored_bytes = BlobStorePort::get(store.as_ref(), &ingested.blob_id)
            .await
            .unwrap();
        assert_eq!(hash_of(&stored_bytes), ingested.content_hash);
    }

    #[tokio::test]
    async fn second_ingest_of_same_content_dedups_without_orphan() {
        let tmp = tempfile::tempdir().unwrap();
        let blob_dir = tmp.path().join("blobs");
        let store = Arc::new(FilesystemBlobStore::new(blob_dir.clone()));
        let repo = Arc::new(InMemoryBlobRepo::default());
        let writer = BlobWriter::new(
            store.clone(),
            repo.clone(),
            FixedClock(0),
            Arc::new(BlobLeaseRegistry::isolated()),
        );

        // Two distinct source paths carrying identical bytes.
        let content = b"dedupe me across two paths".to_vec();
        let src1 = tmp.path().join("one.bin");
        let src2 = tmp.path().join("two.bin");
        tokio::fs::write(&src1, &content).await.unwrap();
        tokio::fs::write(&src2, &content).await.unwrap();

        let first = writer.ingest_path(&src1).await.unwrap();
        let second = writer.ingest_path(&src2).await.unwrap();

        // Same content → same identity, and the existing blob is reused.
        assert_eq!(first.content_hash, second.content_hash);
        assert_eq!(first.blob_id, second.blob_id);
        // The redundant second write was discarded: exactly one blob on disk and
        // one row in the repository, no orphan left behind.
        assert_eq!(count_blob_files(&blob_dir), 1);
        assert_eq!(repo.rows.lock().unwrap().len(), 1);
        assert!(src1.exists());
        assert!(src2.exists());
        assert_eq!(repo.rows.lock().unwrap()[0].last_claimed_at_ms, 0);
    }

    #[tokio::test]
    async fn dedup_hit_refreshes_last_claimed_at() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(FilesystemBlobStore::new(tmp.path().join("blobs")));
        let repo = Arc::new(InMemoryBlobRepo::default());
        let bytes = b"same bytes, later claim".to_vec();
        let hash = hash_of(&bytes);
        let first = BlobWriter::new(
            store.clone(),
            repo.clone(),
            FixedClock(1),
            Arc::new(BlobLeaseRegistry::isolated()),
        );
        let original = first.write_if_absent(&hash, &bytes).await.unwrap();
        assert_eq!(repo.rows.lock().unwrap()[0].last_claimed_at_ms, 1);

        let second = BlobWriter::new(
            store,
            repo.clone(),
            FixedClock(50),
            Arc::new(BlobLeaseRegistry::isolated()),
        );
        let reused = second.write_if_absent(&hash, &bytes).await.unwrap();
        assert_eq!(reused, original);
        assert_eq!(repo.rows.lock().unwrap()[0].last_claimed_at_ms, 50);
        assert_eq!(repo.rows.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dedup_hit_whose_row_vanished_writes_a_new_blob_id() {
        let tmp = tempfile::tempdir().unwrap();
        let blob_dir = tmp.path().join("blobs");
        let store = Arc::new(FilesystemBlobStore::new(blob_dir.clone()));
        let bytes = b"row disappears between find and claim".to_vec();
        let hash = hash_of(&bytes);
        let stale_id = BlobId::from("stale-blob");
        let inner = InMemoryBlobRepo::default();
        inner
            .insert_blob(&Blob::new(
                stale_id.clone(),
                crate::blob::BlobStorageLocator::new_local_fs(blob_dir.join("stale-blob")),
                bytes.len() as i64,
                hash.clone(),
                1,
                1,
                None,
            ))
            .await
            .unwrap();
        let repo = Arc::new(VanishOnClaimRepo {
            inner,
            vanish: AtomicBool::new(true),
        });
        let writer = BlobWriter::new(
            store,
            repo.clone(),
            FixedClock(80),
            Arc::new(BlobLeaseRegistry::isolated()),
        );
        let written = writer.write_if_absent(&hash, &bytes).await.unwrap();
        assert_ne!(written, stale_id);
        assert!(blob_dir.join(written.as_ref()).exists());
        assert!(!blob_dir.join(stale_id.as_ref()).exists());
        let rows = repo.inner.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].blob_id, written);
    }

    #[tokio::test]
    async fn insert_race_discards_the_new_file_and_keeps_the_winner() {
        let tmp = tempfile::tempdir().unwrap();
        let blob_dir = tmp.path().join("blobs");
        let store = Arc::new(FilesystemBlobStore::new(blob_dir.clone()));
        let bytes = b"race winner already stored".to_vec();
        let hash = hash_of(&bytes);
        let winner = BlobId::from("winner-blob");
        let repo = Arc::new(InsertRaceRepo {
            winner: Blob::new(
                winner.clone(),
                crate::blob::BlobStorageLocator::new_local_fs(blob_dir.join("winner-blob")),
                bytes.len() as i64,
                hash.clone(),
                1,
                1,
                None,
            ),
        });
        tokio::fs::create_dir_all(&blob_dir).await.unwrap();
        tokio::fs::write(blob_dir.join("winner-blob"), &bytes)
            .await
            .unwrap();
        let writer = BlobWriter::new(
            store,
            repo,
            FixedClock(3),
            Arc::new(BlobLeaseRegistry::isolated()),
        );
        let written = writer.write_if_absent(&hash, &bytes).await.unwrap();
        assert_eq!(written, winner);
        let mut names = std::fs::read_dir(&blob_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["winner-blob".to_string()]);
    }

    struct InsertRaceRepo {
        winner: Blob,
    }

    #[async_trait]
    impl BlobRepositoryPort for InsertRaceRepo {
        async fn insert_blob(&self, _blob: &Blob) -> Result<()> {
            anyhow::bail!("UNIQUE constraint failed: blob.content_hash");
        }

        async fn find_by_hash(&self, _content_hash: &ContentHash) -> Result<Option<Blob>> {
            Ok(None)
        }

        async fn claim_existing(
            &self,
            content_hash: &ContentHash,
            claimed_at_ms: i64,
        ) -> Result<Option<Blob>> {
            if &self.winner.content_hash != content_hash {
                return Ok(None);
            }
            let mut winner = self.winner.clone();
            winner.last_claimed_at_ms = claimed_at_ms;
            Ok(Some(winner))
        }

        async fn delete_if_unreferenced(
            &self,
            _blob_id: &BlobId,
            _claimed_before_ms: i64,
        ) -> Result<UnreferencedBlobDeletion> {
            Ok(UnreferencedBlobDeletion::Retained)
        }
    }

    struct ClaimFailsRepo {
        existing: Blob,
        fail_on_claim: bool,
    }

    #[async_trait]
    impl BlobRepositoryPort for ClaimFailsRepo {
        async fn insert_blob(&self, blob: &Blob) -> Result<()> {
            if self.fail_on_claim {
                anyhow::bail!("insert failed");
            }
            let _ = blob;
            Ok(())
        }
        async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>> {
            if self.fail_on_claim && &self.existing.content_hash == content_hash {
                return Ok(Some(self.existing.clone()));
            }
            Ok(None)
        }
        async fn claim_existing(&self, _: &ContentHash, _: i64) -> Result<Option<Blob>> {
            anyhow::bail!("claim failed");
        }
        async fn delete_if_unreferenced(
            &self,
            _: &BlobId,
            _: i64,
        ) -> Result<UnreferencedBlobDeletion> {
            Ok(UnreferencedBlobDeletion::Retained)
        }
    }

    #[tokio::test]
    async fn claim_error_before_put_releases_the_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let blob_dir = tmp.path().join("blobs");
        let store = Arc::new(FilesystemBlobStore::new(blob_dir.clone()));
        let bytes = b"claim fails before a new file".to_vec();
        let hash = hash_of(&bytes);
        let existing = BlobId::from("existing-blob");
        let leases = Arc::new(BlobLeaseRegistry::isolated());
        let writer = BlobWriter::new(
            store,
            Arc::new(ClaimFailsRepo {
                existing: Blob::new(
                    existing.clone(),
                    crate::blob::BlobStorageLocator::new_local_fs(blob_dir.join("existing-blob")),
                    bytes.len() as i64,
                    hash.clone(),
                    1,
                    1,
                    None,
                ),
                fail_on_claim: true,
            }),
            FixedClock(4),
            Arc::clone(&leases),
        );
        let error = writer.write_if_absent(&hash, &bytes).await.unwrap_err();
        assert!(error.to_string().contains("claim failed"));
        assert!(!leases.holds(&existing));
        assert_eq!(leases.holder_count(), 0);
        assert_eq!(count_blob_files(&blob_dir), 0);
    }

    struct ClaimAfterInsertFails {
        winner_hash: ContentHash,
    }

    #[async_trait]
    impl BlobRepositoryPort for ClaimAfterInsertFails {
        async fn insert_blob(&self, _: &Blob) -> Result<()> {
            anyhow::bail!("insert failed");
        }
        async fn find_by_hash(&self, _: &ContentHash) -> Result<Option<Blob>> {
            Ok(None)
        }
        async fn claim_existing(&self, content_hash: &ContentHash, _: i64) -> Result<Option<Blob>> {
            assert_eq!(content_hash, &self.winner_hash);
            anyhow::bail!("claim after insert failed");
        }
        async fn delete_if_unreferenced(
            &self,
            _: &BlobId,
            _: i64,
        ) -> Result<UnreferencedBlobDeletion> {
            Ok(UnreferencedBlobDeletion::Retained)
        }
    }

    #[tokio::test]
    async fn claim_error_after_put_discards_the_file_and_releases_the_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let blob_dir = tmp.path().join("blobs");
        let store = Arc::new(FilesystemBlobStore::new(blob_dir.clone()));
        let bytes = b"claim fails after the file exists".to_vec();
        let hash = hash_of(&bytes);
        let leases = Arc::new(BlobLeaseRegistry::isolated());
        let writer = BlobWriter::new(
            store,
            Arc::new(ClaimAfterInsertFails {
                winner_hash: hash.clone(),
            }),
            FixedClock(5),
            Arc::clone(&leases),
        );
        let error = writer.write_if_absent(&hash, &bytes).await.unwrap_err();
        assert!(error.to_string().contains("claim after insert failed"));
        assert_eq!(leases.holder_count(), 0);
        assert_eq!(count_blob_files(&blob_dir), 0);
    }

    struct MismatchClaimRepo {
        looked_up: Blob,
        claimed: Blob,
    }

    #[async_trait]
    impl BlobRepositoryPort for MismatchClaimRepo {
        async fn insert_blob(&self, _: &Blob) -> Result<()> {
            anyhow::bail!("unused");
        }
        async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>> {
            if &self.looked_up.content_hash == content_hash {
                Ok(Some(self.looked_up.clone()))
            } else {
                Ok(None)
            }
        }
        async fn claim_existing(
            &self,
            content_hash: &ContentHash,
            claimed_at_ms: i64,
        ) -> Result<Option<Blob>> {
            if &self.claimed.content_hash != content_hash {
                return Ok(None);
            }
            let mut claimed = self.claimed.clone();
            claimed.last_claimed_at_ms = claimed_at_ms;
            Ok(Some(claimed))
        }
        async fn delete_if_unreferenced(
            &self,
            _: &BlobId,
            _: i64,
        ) -> Result<UnreferencedBlobDeletion> {
            Ok(UnreferencedBlobDeletion::Retained)
        }
    }

    #[tokio::test]
    async fn looked_up_lease_is_released_when_claim_returns_a_different_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(FilesystemBlobStore::new(tmp.path().join("blobs")));
        let bytes = b"lookup id and claim id differ".to_vec();
        let hash = hash_of(&bytes);
        let looked_up = BlobId::from("looked-up");
        let claimed = BlobId::from("claimed");
        let leases = Arc::new(BlobLeaseRegistry::isolated());
        let locator = crate::blob::BlobStorageLocator::new_local_fs(tmp.path().join("row"));
        let writer = BlobWriter::new(
            store,
            Arc::new(MismatchClaimRepo {
                looked_up: Blob::new(
                    looked_up.clone(),
                    locator.clone(),
                    bytes.len() as i64,
                    hash.clone(),
                    1,
                    1,
                    None,
                ),
                claimed: Blob::new(
                    claimed.clone(),
                    locator,
                    bytes.len() as i64,
                    hash.clone(),
                    1,
                    1,
                    None,
                ),
            }),
            FixedClock(6),
            Arc::clone(&leases),
        );
        let written = writer.write_if_absent(&hash, &bytes).await.unwrap();
        assert_eq!(written, claimed);
        assert!(!leases.holds(&looked_up));
        assert!(leases.holds(&claimed));
        leases.release(&claimed);
        assert_eq!(leases.holder_count(), 0);
    }
}
