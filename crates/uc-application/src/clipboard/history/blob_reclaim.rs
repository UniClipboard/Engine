//! 删除或替换条目后，回收已经没有任何引用的 vault blob。
//!
//! 调用方只提交候选 `blob_id`。是否删除由实现在数据库事务里按三类引用判断。
//! 文件删除失败留在报告里，不回滚已经提交的条目删除。

use async_trait::async_trait;
use uc_core::BlobId;

/// 一次回收调用的计数。失败项不撤销已经删除的行。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlobReclaimReport {
    pub checked: u64,
    pub rows_deleted: u64,
    pub files_deleted: u64,
    pub bytes_freed: u64,
    pub missing_files: u64,
    pub failures: u64,
    /// 无引用但 `last_claimed_at_ms` 仍在宽限内，留给 PR2（WF-engine-39 GC sweep）。
    pub skipped_recently_claimed: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("unreferenced blob reclaim failed")]
pub struct BlobReclaimError {
    #[source]
    source: anyhow::Error,
}

impl BlobReclaimError {
    pub fn storage(source: anyhow::Error) -> Self {
        Self { source }
    }
}

#[async_trait]
pub trait BlobReclaimPort: Send + Sync {
    /// 对候选 blob 做无引用回收。单个失败计入 [`BlobReclaimReport::failures`]，
    /// 只有回收过程本身无法继续时才返回错误。
    async fn reclaim_unreferenced(
        &self,
        blob_ids: &[BlobId],
    ) -> Result<BlobReclaimReport, BlobReclaimError>;

    /// 引用已经落库，或本次写入失败，释放这些 blob 的进程内写入租约。
    ///
    /// 租约只减少同进程竞争，按持有者计数。多释放一次没有对应租约时不做任何事。
    /// 未释放的租约在 TTL 后过期，该时长需实测确认。跨进程窗口由认领宽限保证，
    /// 宽限内的无引用 blob 记入 [`BlobReclaimReport::skipped_recently_claimed`]。
    fn release_write_leases(&self, blob_ids: &[BlobId]);
}
