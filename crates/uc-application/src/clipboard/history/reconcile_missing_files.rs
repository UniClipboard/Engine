//! Startup reconciliation: drop DB entries whose backing cache files have
//! vanished.
//!
//! Companion to [`super::cleanup::CleanupExpiredFilesUseCase`]. Cleanup
//! walks the file-cache directory to find files past their retention TTL
//! and matches them back to entries. Reconcile walks the entry list and
//! drops any entry whose cache-managed `file://` paths no longer exist on
//! disk. The two passes together close both directions of cache↔DB drift.
//!
//! Why this exists: under iroh-blobs 0.100.0 a `Complete{External(path)}`
//! entry whose path has been removed externally triggers
//! `BaoFileStorage::open -> Err(_) -> Poisoned`, and a subsequent
//! `observe(hash)` panics on `bao_file.rs:410` with
//! "poisoned storage should not be used". Old uniclipboard releases ran a
//! raw `tokio::fs::remove_file` cleanup that left exactly this drift behind;
//! reconcile cleans up that historical mess on startup so the upstream
//! crate never sees a stale External path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use uc_core::ids::{EntryId, RepresentationId};
use uc_core::ports::CacheFsPort;
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_info, uc_warn,
};

use super::delete_entry::DeleteClipboardEntryUseCase;
use super::file_references::HistoryFileReferencePort;

/// Result of a reconcile pass.
#[derive(Debug, Default, Clone)]
pub struct ReconcileResult {
    /// Number of entries scanned across every batch.
    pub entries_scanned: u32,
    /// Number of entries dropped because at least one of their cache files
    /// was missing on disk.
    pub entries_deleted: u32,
    /// Number of entries that we *wanted* to drop but `delete_entry`
    /// failed on. Surfaced separately so callers can decide whether to
    /// alert vs. silently log.
    pub errors: u32,
}

const ENTRY_LIST_BATCH_SIZE: usize = 1000;

pub(crate) struct ReconcileMissingFilesUseCase {
    file_cache_dir: PathBuf,
    file_references: Arc<dyn HistoryFileReferencePort>,
    delete_uc: Arc<DeleteClipboardEntryUseCase>,
    cache_fs: Arc<dyn CacheFsPort>,
}

impl ReconcileMissingFilesUseCase {
    pub(crate) fn new(
        file_cache_dir: PathBuf,
        file_references: Arc<dyn HistoryFileReferencePort>,
        delete_uc: Arc<DeleteClipboardEntryUseCase>,
        cache_fs: Arc<dyn CacheFsPort>,
    ) -> Self {
        Self {
            file_cache_dir,
            file_references,
            delete_uc,
            cache_fs,
        }
    }

    #[tracing::instrument(name = "usecase.reconcile_missing_files.execute", skip_all)]
    pub(crate) async fn execute(&self) -> Result<ReconcileResult> {
        if !self.cache_fs.exists(&self.file_cache_dir).await {
            uc_info!("File cache directory does not exist, nothing to reconcile");
            return Ok(ReconcileResult::default());
        }

        let mut result = ReconcileResult::default();
        let mut after: Option<RepresentationId> = None;
        let mut handled: HashSet<EntryId> = HashSet::new();
        let mut to_delete: Vec<EntryId> = Vec::new();

        // 先扫描再删除；文件引用按稳定游标分页，不读取无关正文。
        loop {
            let batch = self
                .file_references
                .list_file_references(after.as_ref(), ENTRY_LIST_BATCH_SIZE)
                .await
                .context("read history file references for reconciliation")?;

            if batch.is_empty() {
                break;
            }
            let batch_len = batch.len();
            after = batch.last().map(|item| item.representation_id.clone());
            for item in &batch {
                if handled.insert(item.entry_id.clone()) {
                    result.entries_scanned += 1;
                }
                for path in extract_cache_paths(&item.uri_list, &self.file_cache_dir) {
                    if !self.cache_fs.exists(&path).await {
                        if !to_delete.contains(&item.entry_id) {
                            to_delete.push(item.entry_id.clone());
                        }
                        break;
                    }
                }
            }

            if batch_len < ENTRY_LIST_BATCH_SIZE {
                break;
            }
        }

        for entry_id in &to_delete {
            match self.delete_uc.execute(entry_id).await {
                Ok(()) => {
                    result.entries_deleted += 1;
                    uc_info!(
                        entry_id = log_id(&entry_id),
                        "Reconcile: dropped entry whose cache file no longer exists"
                    );
                }
                Err(e) => {
                    uc_warn!(
                        entry_id = log_id(&entry_id),
                        error_kind = "entry_delete",
                        io_error_kind = io_error_kind(e.as_ref()),
                        "Reconcile: delete_entry failed for stale entry"
                    );
                    result.errors += 1;
                }
            }
        }

        uc_info!(
            entries_scanned = result.entries_scanned,
            entries_deleted = result.entries_deleted,
            errors = result.errors,
            "Reconcile pass complete"
        );
        Ok(result)
    }
}

/// Decode a `text/uri-list` payload and return only those `file://` paths
/// that fall under `cache_dir`. Lines that fail to parse, comments, and
/// paths outside `cache_dir` are silently skipped — they are not the
/// reconcile target. This mirrors the extraction logic in
/// [`super::cleanup`] and [`super::delete_entry`], lifted into a pure
/// helper so it can be unit-tested without a full use-case fixture.
fn extract_cache_paths(uri_list: &[u8], cache_dir: &Path) -> Vec<PathBuf> {
    let text = String::from_utf8_lossy(uri_list);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parsed = if line.starts_with("file://") {
            url::Url::parse(line)
                .ok()
                .and_then(|u| u.to_file_path().ok())
        } else {
            Some(PathBuf::from(line))
        };
        let Some(path) = parsed else { continue };
        if path.starts_with(cache_dir) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_dir() -> PathBuf {
        PathBuf::from("/var/cache/uniclipboard")
    }

    #[test]
    fn extracts_file_uri_inside_cache_dir() {
        let uri = b"file:///var/cache/uniclipboard/abc/IMG.JPG\n";
        let paths = extract_cache_paths(uri, &cache_dir());
        assert_eq!(
            paths,
            vec![PathBuf::from("/var/cache/uniclipboard/abc/IMG.JPG")]
        );
    }

    #[test]
    fn skips_paths_outside_cache_dir() {
        // user dropped a file from their Downloads folder — we never
        // managed it, so it must not show up in the reconcile target set.
        let uri = b"file:///Users/alice/Downloads/IMG.JPG\n";
        let paths = extract_cache_paths(uri, &cache_dir());
        assert!(paths.is_empty(), "user-owned path should be filtered out");
    }

    #[test]
    fn skips_blank_lines_and_comments() {
        let uri = b"# this is a comment\n\nfile:///var/cache/uniclipboard/a/b.txt\n\n";
        let paths = extract_cache_paths(uri, &cache_dir());
        assert_eq!(
            paths,
            vec![PathBuf::from("/var/cache/uniclipboard/a/b.txt")]
        );
    }

    #[test]
    fn accepts_bare_path_lines() {
        // Some legacy representations carry plain paths instead of
        // file:// URIs. The reverse-index loop in the original cleanup
        // also handled this; keep parity.
        let uri = b"/var/cache/uniclipboard/abc/file.bin\n";
        let paths = extract_cache_paths(uri, &cache_dir());
        assert_eq!(
            paths,
            vec![PathBuf::from("/var/cache/uniclipboard/abc/file.bin")]
        );
    }

    #[test]
    fn ignores_garbage_lines() {
        let uri = b"not a url\nfile://?garbage\nfile:///var/cache/uniclipboard/ok.txt\n";
        let paths = extract_cache_paths(uri, &cache_dir());
        // The bare-path branch (`Some(PathBuf::from(line))`) means
        // unparseable lines that aren't `file://`-prefixed still become
        // a PathBuf and are filtered by the cache-dir prefix check.
        // Only the legitimate cache path should survive.
        assert_eq!(paths, vec![PathBuf::from("/var/cache/uniclipboard/ok.txt")]);
    }

    #[test]
    fn returns_each_path_in_multi_file_payload() {
        let uri =
            b"file:///var/cache/uniclipboard/a/1.txt\nfile:///var/cache/uniclipboard/b/2.txt\n";
        let paths = extract_cache_paths(uri, &cache_dir());
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/var/cache/uniclipboard/a/1.txt"),
                PathBuf::from("/var/cache/uniclipboard/b/2.txt"),
            ]
        );
    }

    struct ReconcileReclaim {
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::clipboard::history::blob_reclaim::BlobReclaimPort for ReconcileReclaim {
        async fn reclaim_unreferenced(
            &self,
            blob_ids: &[uc_core::BlobId],
        ) -> Result<
            crate::clipboard::history::blob_reclaim::BlobReclaimReport,
            crate::clipboard::history::blob_reclaim::BlobReclaimError,
        > {
            self.seen
                .lock()
                .unwrap()
                .extend(blob_ids.iter().map(|id| id.to_string()));
            Ok(Default::default())
        }
        fn release_write_leases(&self, _: &[uc_core::BlobId]) {}
    }

    struct ReconcileRefs {
        item: super::super::file_references::HistoryFileReference,
    }

    #[async_trait::async_trait]
    impl super::super::file_references::HistoryFileReferencePort for ReconcileRefs {
        async fn list_file_references(
            &self,
            after: Option<&uc_core::ids::RepresentationId>,
            _: usize,
        ) -> Result<Vec<super::super::file_references::HistoryFileReference>> {
            if after.is_some() {
                Ok(Vec::new())
            } else {
                Ok(vec![self.item.clone()])
            }
        }
    }

    struct ReconcileStore;

    #[async_trait::async_trait]
    impl uc_core::ports::clipboard::GetClipboardEntryPort for ReconcileStore {
        async fn get_entry(
            &self,
            _: &EntryId,
        ) -> Result<Option<uc_core::ClipboardEntry>, uc_core::clipboard::ClipboardRepositoryError>
        {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl uc_core::ports::clipboard::ListRepresentationsForEventPort for ReconcileStore {
        async fn get_representations_for_event(
            &self,
            _: &uc_core::ids::EventId,
        ) -> Result<
            Vec<uc_core::clipboard::PersistedClipboardRepresentation>,
            uc_core::clipboard::ClipboardRepositoryError,
        > {
            Ok(Vec::new())
        }
    }

    #[async_trait::async_trait]
    impl uc_core::ports::clipboard::DeleteClipboardEntryWithReceiveStatePort for ReconcileStore {
        async fn delete_entry_with_receive_state(
            &self,
            _: &EntryId,
        ) -> Result<
            uc_core::ports::clipboard::ClipboardEntryRemoval,
            uc_core::clipboard::ClipboardRepositoryError,
        > {
            Ok(uc_core::ports::clipboard::ClipboardEntryRemoval {
                removed: true,
                candidate_blob_ids: vec![uc_core::BlobId::from("blob-reconcile")],
            })
        }
    }

    struct PresentCacheDir {
        cache_dir: PathBuf,
    }

    #[async_trait::async_trait]
    impl CacheFsPort for PresentCacheDir {
        async fn exists(&self, path: &Path) -> bool {
            path == self.cache_dir
        }
        async fn read_dir(&self, _: &Path) -> Result<Vec<uc_core::ports::cache_fs::DirEntry>> {
            Ok(Vec::new())
        }
        async fn remove_dir_all(&self, _: &Path) -> Result<()> {
            Ok(())
        }
        async fn remove_file(&self, _: &Path) -> Result<()> {
            Ok(())
        }
        async fn dir_size(&self, _: &Path) -> Result<u64> {
            Ok(0)
        }
        async fn read_file(&self, _: &Path) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        async fn write_file(&self, _: &Path, _: &[u8]) -> Result<()> {
            Ok(())
        }
        async fn metadata(
            &self,
            _: &Path,
        ) -> Result<Option<uc_core::ports::cache_fs::FileMetadata>> {
            Ok(None)
        }
        async fn remove_dir(&self, _: &Path) -> Result<()> {
            Ok(())
        }
    }

    struct ZeroClock;
    impl crate::connectivity::ElapsedClockPort for ZeroClock {
        fn elapsed(&self) -> Result<std::time::Duration, crate::connectivity::ElapsedClockError> {
            Ok(std::time::Duration::ZERO)
        }
    }

    #[tokio::test]
    async fn execute_reclaims_the_blob_of_a_missing_cache_file() {
        use crate::clipboard::history::delete_entry::DeleteClipboardEntryUseCase;
        use crate::transfer::progress::ProgressProjectionRegistry;
        use crate::transfer::receive::work::ReceiveWorkRegistry;

        let cache_dir = PathBuf::from("/var/cache/uniclipboard");
        let missing = cache_dir.join("gone.bin");
        let uri = format!("file://{}\n", missing.display()).into_bytes();
        let refs = Arc::new(ReconcileRefs {
            item: super::super::file_references::HistoryFileReference {
                entry_id: EntryId::from("stale"),
                representation_id: uc_core::ids::RepresentationId::from("rep-stale"),
                uri_list: uri,
            },
        });
        let store = Arc::new(ReconcileStore);
        let reclaim = Arc::new(ReconcileReclaim {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let work = Arc::new(ReceiveWorkRegistry::default());
        let (progress, _worker) = ProgressProjectionRegistry::new(Arc::new(ZeroClock));
        let delete_uc = Arc::new(DeleteClipboardEntryUseCase::from_ports(
            store.clone(),
            store.clone(),
            store,
            work,
            progress,
            reclaim.clone(),
        ));
        let usecase = ReconcileMissingFilesUseCase::new(
            cache_dir.clone(),
            refs,
            delete_uc,
            Arc::new(PresentCacheDir { cache_dir }),
        );
        let result = usecase.execute().await.unwrap();
        assert_eq!(result.entries_deleted, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(
            reclaim.seen.lock().unwrap().as_slice(),
            ["blob-reconcile".to_string()]
        );
    }
}
