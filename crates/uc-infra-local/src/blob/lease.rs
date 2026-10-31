//! 进程内 blob 写入租约。
//!
//! 只减少同一进程里“正在写入”和“正在回收”的竞争。跨进程正确性不依赖它：
//! 删除是否安全由 `delete_if_unreferenced` 的引用检查决定，去重是否复用旧 id
//! 由认领事务确认行仍存在决定。
//!
//! 作用域按 profile 在进程内共享，不按 Engine 实例分开。iOS 主 App 与键盘扩展
//! 是不同进程，它们之间的窗口仍由数据库事务保证。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use uc_core::BlobId;

/// 同进程写入到引用落库之间的保守上界。
///
/// 15 分钟长于一次正常捕获，用来避免租约在引用落库前过期。
/// 该时长需实测确认。捕获成功后会主动释放，因此正常删除不会被这 15 分钟挡住。
pub const BLOB_LEASE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
pub struct BlobLeaseRegistry {
    leases: Mutex<HashMap<String, Instant>>,
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

    pub fn acquire(&self, blob_id: &BlobId) {
        let mut guard = self
            .leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(blob_id.inner().clone(), Instant::now() + BLOB_LEASE_TTL);
    }

    pub fn release(&self, blob_id: &BlobId) {
        let mut guard = self
            .leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(blob_id.inner());
    }

    pub fn holds(&self, blob_id: &BlobId) -> bool {
        let mut guard = self
            .leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        guard.retain(|_, expiry| *expiry > now);
        guard.contains_key(blob_id.inner())
    }
}
