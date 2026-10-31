//! 进程内 blob 写入租约。
//!
//! 只减少同一进程里“正在写入”和“正在回收”的竞争。跨进程正确性不依赖这份表，
//! 由三项共同保证：
//! 1. `delete_if_unreferenced` 在 `BEGIN IMMEDIATE` 事务里检查三类 `NOT EXISTS`；
//! 2. 同一事务还要求 `last_claimed_at_ms` 早于认领宽限；
//! 3. 写入 `entry_file_set.blob_id` 时在同一事务里确认 blob 行仍在。
//!
//! 租约按持有者计数。同一 blob 被两个写入者同时命中时，一次释放不会清掉另一个。
//! 作用域按 profile 在进程内共享。iOS 主 App 与键盘扩展是不同进程，不共享这份表。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use uc_core::BlobId;

/// 同进程写入租约的保守上界。
///
/// 15 分钟只覆盖一次正常捕获。过期条目即使持有者计数还在也会被丢掉，
/// 避免泄漏的计数永远挡住回收。跨进程的认领窗口由
/// [`BLOB_RECLAIM_CLAIM_GRACE`] 保证，不由这个时长保证。该时长需实测确认。
pub const BLOB_LEASE_TTL: Duration = Duration::from_secs(15 * 60);

/// 认领到引用提交之间的保守上界。
///
/// 删除无引用 blob 时，`last_claimed_at_ms` 必须早于现在减去这个时长。
/// 60 分钟覆盖后台物化队列，以及达到产品解压上限（500 MiB）的大文件压缩与加密。
/// 15 分钟的进程内租约只覆盖一次正常捕获，盖不住排队中的大文件。
/// 该时长需实测确认。
///
/// 新插入的 blob 把 `last_claimed_at_ms` 初始化为插入时间，因此「复制后立刻删除」
/// 会落在宽限内，留给 PR2（WF-engine-39 GC sweep）。正确性优先于立刻回收。
pub const BLOB_RECLAIM_CLAIM_GRACE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
struct LeaseHold {
    holders: u32,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct BlobLeaseRegistry {
    leases: Mutex<HashMap<String, LeaseHold>>,
}

/// 持有一次 `acquire`。`Drop` 时释放；[`BlobLeaseGuard::persist`] 把计数留给调用方。
pub struct BlobLeaseGuard {
    registry: Arc<BlobLeaseRegistry>,
    blob_id: BlobId,
    armed: bool,
}

impl BlobLeaseGuard {
    pub fn blob_id(&self) -> &BlobId {
        &self.blob_id
    }

    /// 引用尚未落库，计数留给调用方在成功或失败路径上释放。
    pub fn persist(mut self) {
        self.armed = false;
    }
}

impl Drop for BlobLeaseGuard {
    fn drop(&mut self) {
        if self.armed {
            self.registry.release(&self.blob_id);
        }
    }
}

impl BlobLeaseRegistry {
    pub fn isolated() -> Self {
        Self {
            leases: Mutex::new(HashMap::new()),
        }
    }

    /// 同一 profile、同一进程里的 Writer 和 Reclaimer 必须拿到同一份注册表。
    pub fn for_profile(profile_key: &str) -> Arc<Self> {
        static REGISTRIES: OnceLock<Mutex<HashMap<String, Arc<BlobLeaseRegistry>>>> =
            OnceLock::new();
        let registries = REGISTRIES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = registries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .entry(profile_key.to_owned())
            .or_insert_with(|| Arc::new(Self::isolated()))
            .clone()
    }

    pub fn acquire_guard(self: &Arc<Self>, blob_id: &BlobId) -> BlobLeaseGuard {
        self.acquire(blob_id);
        BlobLeaseGuard {
            registry: Arc::clone(self),
            blob_id: blob_id.clone(),
            armed: true,
        }
    }

    pub fn acquire(&self, blob_id: &BlobId) {
        let mut guard = self.lock();
        let now = Instant::now();
        let key = blob_id.inner().clone();
        match guard.get_mut(&key) {
            Some(hold) if hold.expires_at > now => {
                hold.holders = hold.holders.saturating_add(1);
                hold.expires_at = now + BLOB_LEASE_TTL;
            }
            _ => {
                guard.insert(
                    key,
                    LeaseHold {
                        holders: 1,
                        expires_at: now + BLOB_LEASE_TTL,
                    },
                );
            }
        }
    }

    /// 少一次持有者。没有对应租约时不做任何事，因此多释放一次是安全的。
    pub fn release(&self, blob_id: &BlobId) {
        let mut guard = self.lock();
        let Some(hold) = guard.get_mut(blob_id.inner()) else {
            return;
        };
        hold.holders = hold.holders.saturating_sub(1);
        if hold.holders == 0 {
            guard.remove(blob_id.inner());
        }
    }

    pub fn holds(&self, blob_id: &BlobId) -> bool {
        let mut guard = self.lock();
        let now = Instant::now();
        guard.retain(|_, hold| hold.expires_at > now);
        guard.contains_key(blob_id.inner())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, LeaseHold>> {
        self.leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn holder_count(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_holders_survive_one_release() {
        let registry = Arc::new(BlobLeaseRegistry::isolated());
        let blob_id = BlobId::from("shared");
        registry.acquire(&blob_id);
        registry.acquire(&blob_id);
        registry.release(&blob_id);
        assert!(registry.holds(&blob_id));
        registry.release(&blob_id);
        assert!(!registry.holds(&blob_id));
        registry.release(&blob_id);
        assert_eq!(registry.holder_count(), 0);
    }

    #[test]
    fn guard_drop_releases_and_persist_keeps_the_count() {
        let registry = Arc::new(BlobLeaseRegistry::isolated());
        let blob_id = BlobId::from("held");
        {
            let guard = registry.acquire_guard(&blob_id);
            assert!(registry.holds(&blob_id));
            drop(guard);
        }
        assert!(!registry.holds(&blob_id));

        let guard = registry.acquire_guard(&blob_id);
        guard.persist();
        assert!(registry.holds(&blob_id));
        registry.release(&blob_id);
        assert!(!registry.holds(&blob_id));
    }
}
