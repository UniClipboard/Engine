//! 显式原生资源验收的技术计数；不返回连接、标识或业务状态，不属于产品接口。
use super::blobs::BLOBS_ALPN;
use futures_util::StreamExt;
use iroh::endpoint::{Connection, WeakConnectionHandle};
use iroh_blobs::store::fs::FsStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

static NATIVE_HANDLES: Mutex<Vec<WeakConnectionHandle>> = Mutex::new(Vec::new());
static TAG_REPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static NATIVE_OBSERVERS: AtomicUsize = AtomicUsize::new(0);

/// 测试显式观察新增原生句柄；只持弱引用，不修改进程环境或产品接口。
pub struct BlobNativeObservation {
    initial: usize,
}
impl BlobNativeObservation {
    pub fn start() -> Self {
        let handles = NATIVE_HANDLES
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        NATIVE_OBSERVERS.fetch_add(1, Ordering::SeqCst);
        Self {
            initial: handles.len(),
        }
    }
    pub fn counts(&self) -> (usize, usize) {
        let handles = NATIVE_HANDLES
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let observed = &handles[self.initial..];
        (
            observed.len(),
            observed
                .iter()
                .filter(|handle| handle.upgrade().is_some())
                .count(),
        )
    }
}
impl Drop for BlobNativeObservation {
    fn drop(&mut self) {
        NATIVE_OBSERVERS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn register_blob_connection(connection: &Connection) {
    if connection.alpn() == BLOBS_ALPN
        && (std::env::var_os("UC_RECEIVE_RESOURCE_EVIDENCE_DIR").is_some()
            || NATIVE_OBSERVERS.load(Ordering::SeqCst) > 0)
    {
        NATIVE_HANDLES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(connection.weak_handle());
    }
}

/// 已观察端侧原生句柄数与仍存活数；只持弱句柄，不添加任务或延长连接生命。
pub fn blob_native_handle_counts() -> (usize, usize) {
    let handles = NATIVE_HANDLES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    (
        handles.len(),
        handles
            .iter()
            .filter(|handle| handle.upgrade().is_some())
            .count(),
    )
}

pub(crate) async fn record_blob_tags_before_shutdown(store: &FsStore) {
    let Some(directory) = std::env::var_os("UC_RECEIVE_RESOURCE_EVIDENCE_DIR") else {
        return;
    };
    // 由原完整关闭路径调用，此时业务工作和连接已结算，SDK actor仍打开。
    let tags = store
        .tags()
        .list_temp_tags()
        .await
        .expect("读取真实SDK临时tag")
        .count()
        .await;
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).expect("创建SDK资源证据目录");
    let sequence = TAG_REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("sdk-tags-{}-{sequence}.json", std::process::id()));
    let report = serde_json::json!({
        "phase": "before_sdk_store_shutdown",
        "store_open_during_count": true,
        "sdk_temp_tags": tags,
        "contains_only_resource_counts": true,
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report).expect("编码SDK资源计数"),
    )
    .expect("保存SDK资源计数");
}
