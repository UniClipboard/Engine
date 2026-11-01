//! 先删除无引用且已过认领宽限的 blob 行并提交，再删除文件。
//!
//! 文件删除失败只记警告，不把行插回去。留下的文件由 PR2（WF-engine-39 GC sweep）重试。
//! 仍在宽限内的无引用 blob 记入 `skipped_recently_claimed`，同样留给那次扫描。
//! 文件已经不在则视为成功，计入 `missing_files`。

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{BlobReclaimError, BlobReclaimPort, BlobReclaimReport};
use uc_core::ports::ClockPort;
use uc_core::BlobId;
use uc_observability_contract::{error_source::io_error_kind, log_fields::log_id, uc_warn};

use super::lease::{BlobLeaseRegistry, BLOB_RECLAIM_CLAIM_GRACE};
use super::store_port::BlobFileRemoval;
use super::{BlobRepositoryPort, BlobStorePort, UnreferencedBlobDeletion};

pub struct BlobReclaimer<S, R, C> {
    store: S,
    repository: R,
    leases: Arc<BlobLeaseRegistry>,
    clock: C,
}

impl<S, R, C> BlobReclaimer<S, R, C>
where
    S: BlobStorePort,
    R: BlobRepositoryPort,
    C: ClockPort,
{
    pub fn new(store: S, repository: R, leases: Arc<BlobLeaseRegistry>, clock: C) -> Self {
        Self {
            store,
            repository,
            leases,
            clock,
        }
    }
}

fn claimed_before_ms(now_ms: i64) -> i64 {
    let grace_ms = i64::try_from(BLOB_RECLAIM_CLAIM_GRACE.as_millis()).unwrap_or(i64::MAX);
    now_ms.saturating_sub(grace_ms)
}

#[async_trait]
impl<S, R, C> BlobReclaimPort for BlobReclaimer<S, R, C>
where
    S: BlobStorePort,
    R: BlobRepositoryPort,
    C: ClockPort,
{
    async fn reclaim_unreferenced(
        &self,
        blob_ids: &[BlobId],
    ) -> Result<BlobReclaimReport, BlobReclaimError> {
        let mut report = BlobReclaimReport::default();
        let mut seen = BTreeSet::new();
        let claimed_before_ms = claimed_before_ms(self.clock.now_ms());
        for blob_id in blob_ids {
            if !seen.insert(blob_id.inner().clone()) {
                continue;
            }
            report.checked = report.checked.saturating_add(1);
            // 租约只跳过本进程仍在写入、引用尚未落库的 blob。
            if self.leases.holds(blob_id) {
                continue;
            }
            match self
                .repository
                .delete_if_unreferenced(blob_id, claimed_before_ms)
                .await
            {
                Ok(UnreferencedBlobDeletion::Retained) => {}
                Ok(UnreferencedBlobDeletion::RecentlyClaimed) => {
                    report.skipped_recently_claimed =
                        report.skipped_recently_claimed.saturating_add(1);
                }
                Ok(UnreferencedBlobDeletion::Deleted { bytes_on_disk }) => {
                    report.rows_deleted = report.rows_deleted.saturating_add(1);
                    match self.store.remove_stored_file(blob_id).await {
                        Ok(BlobFileRemoval::Deleted) => {
                            report.files_deleted = report.files_deleted.saturating_add(1);
                            report.bytes_freed = report.bytes_freed.saturating_add(bytes_on_disk);
                        }
                        Ok(BlobFileRemoval::Missing) => {
                            report.missing_files = report.missing_files.saturating_add(1);
                        }
                        Err(error) => {
                            report.failures = report.failures.saturating_add(1);
                            uc_warn!(
                                error_kind = "blob_file_remove",
                                io_error_kind = io_error_kind(error.as_ref()),
                                blob_id = log_id(blob_id),
                                "unreferenced blob row was deleted but its file remains for PR2 (WF-engine-39 GC sweep)"
                            );
                        }
                    }
                }
                Err(error) => {
                    report.failures = report.failures.saturating_add(1);
                    uc_warn!(
                        error_kind = "blob_row_remove",
                        io_error_kind = io_error_kind(error.as_ref()),
                        blob_id = log_id(blob_id),
                        "failed to delete an unreferenced blob row"
                    );
                }
            }
        }
        Ok(report)
    }

    fn release_write_leases(&self, blob_ids: &[BlobId]) {
        for blob_id in blob_ids {
            self.leases.release(blob_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BlobReclaimer, BlobRepositoryPort, BlobStorePort};
    use crate::blob::lease::BlobLeaseRegistry;
    use crate::blob::{BlobFileRemoval, StoredPathBlob, UnreferencedBlobDeletion};
    use anyhow::anyhow;
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::Mutex;
    use uc_application::deps::BlobReclaimPort;
    use uc_core::BlobId;
    use uc_core::ContentHash;

    struct Repo {
        events: Arc<Mutex<Vec<&'static str>>>,
        deleted: UnreferencedBlobDeletion,
    }

    #[async_trait]
    impl BlobRepositoryPort for Repo {
        async fn insert_blob(&self, _: &crate::blob::Blob) -> anyhow::Result<()> {
            Err(anyhow!("unused"))
        }
        async fn find_by_hash(&self, _: &ContentHash) -> anyhow::Result<Option<crate::blob::Blob>> {
            Err(anyhow!("unused"))
        }
        async fn claim_existing(
            &self,
            _: &ContentHash,
            _: i64,
        ) -> anyhow::Result<Option<crate::blob::Blob>> {
            Err(anyhow!("unused"))
        }
        async fn delete_if_unreferenced(
            &self,
            _: &BlobId,
            _: i64,
        ) -> anyhow::Result<UnreferencedBlobDeletion> {
            self.events.lock().expect("events").push("row");
            Ok(self.deleted)
        }
    }

    struct Store {
        events: Arc<Mutex<Vec<&'static str>>>,
        removal: Mutex<Result<BlobFileRemoval, &'static str>>,
    }

    #[async_trait]
    impl BlobStorePort for Store {
        async fn put(&self, _: &BlobId, _: &[u8]) -> anyhow::Result<(PathBuf, Option<i64>)> {
            Err(anyhow!("unused"))
        }
        async fn get(&self, _: &BlobId) -> anyhow::Result<Vec<u8>> {
            Err(anyhow!("unused"))
        }
        async fn put_from_path(&self, _: &BlobId, _: &Path) -> anyhow::Result<StoredPathBlob> {
            Err(anyhow!("unused"))
        }
        async fn delete(&self, _: &BlobId) -> anyhow::Result<()> {
            Err(anyhow!("unused"))
        }
        async fn remove_stored_file(&self, _: &BlobId) -> anyhow::Result<BlobFileRemoval> {
            self.events.lock().expect("events").push("file");
            match *self.removal.lock().expect("removal") {
                Ok(removal) => Ok(removal),
                Err(message) => Err(anyhow!(message)),
            }
        }
    }

    struct FixedClock(i64);
    impl uc_core::ports::ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    fn reclaimer(
        deleted: UnreferencedBlobDeletion,
        removal: Result<BlobFileRemoval, &'static str>,
    ) -> (
        BlobReclaimer<Store, Repo, FixedClock>,
        Arc<Mutex<Vec<&'static str>>>,
        Arc<BlobLeaseRegistry>,
    ) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let leases = Arc::new(BlobLeaseRegistry::isolated());
        let reclaimer = BlobReclaimer::new(
            Store {
                events: Arc::clone(&events),
                removal: Mutex::new(removal),
            },
            Repo {
                events: Arc::clone(&events),
                deleted,
            },
            Arc::clone(&leases),
            FixedClock(0),
        );
        (reclaimer, events, leases)
    }

    #[tokio::test]
    async fn leased_blob_is_skipped_without_deleting_the_row() {
        let (reclaimer, events, leases) = reclaimer(
            UnreferencedBlobDeletion::Deleted { bytes_on_disk: 4 },
            Ok(BlobFileRemoval::Deleted),
        );
        let blob_id = BlobId::from("held");
        leases.acquire(&blob_id);
        let report = BlobReclaimPort::reclaim_unreferenced(&reclaimer, &[blob_id])
            .await
            .unwrap();
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(report.checked, 1);
        assert_eq!(report.rows_deleted, 0);
        assert_eq!(report.failures, 0);
    }

    #[tokio::test]
    async fn row_is_committed_before_the_file_is_removed() {
        let (reclaimer, events, _) = reclaimer(
            UnreferencedBlobDeletion::Deleted { bytes_on_disk: 9 },
            Ok(BlobFileRemoval::Deleted),
        );
        let report = BlobReclaimPort::reclaim_unreferenced(&reclaimer, &[BlobId::from("gone")])
            .await
            .unwrap();
        assert_eq!(events.lock().unwrap().as_slice(), ["row", "file"]);
        assert_eq!(report.rows_deleted, 1);
        assert_eq!(report.files_deleted, 1);
        assert_eq!(report.bytes_freed, 9);
    }

    #[tokio::test]
    async fn missing_file_counts_as_success() {
        let (reclaimer, _, _) = reclaimer(
            UnreferencedBlobDeletion::Deleted { bytes_on_disk: 9 },
            Ok(BlobFileRemoval::Missing),
        );
        let report = BlobReclaimPort::reclaim_unreferenced(&reclaimer, &[BlobId::from("gone")])
            .await
            .unwrap();
        assert_eq!(report.rows_deleted, 1);
        assert_eq!(report.files_deleted, 0);
        assert_eq!(report.missing_files, 1);
        assert_eq!(report.failures, 0);
        assert_eq!(report.bytes_freed, 0);
    }

    #[tokio::test]
    async fn recently_claimed_blob_is_counted_and_its_file_stays() {
        let (reclaimer, events, _) = reclaimer(
            UnreferencedBlobDeletion::RecentlyClaimed,
            Ok(BlobFileRemoval::Deleted),
        );
        let report = BlobReclaimPort::reclaim_unreferenced(&reclaimer, &[BlobId::from("fresh")])
            .await
            .unwrap();
        assert_eq!(events.lock().unwrap().as_slice(), ["row"]);
        assert_eq!(report.checked, 1);
        assert_eq!(report.skipped_recently_claimed, 1);
        assert_eq!(report.rows_deleted, 0);
        assert_eq!(report.files_deleted, 0);
        assert_eq!(report.failures, 0);
    }
}
