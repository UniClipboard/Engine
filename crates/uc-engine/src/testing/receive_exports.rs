//! 真实接收导出边界的存储故障与完成屏障；只由测试组合根安装，不替换网络或保存流程。
use async_trait::async_trait;
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Notify, Semaphore};
use tokio_util::bytes::Bytes;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobReadStorePort, BlobReceived,
};
use uc_core::ports::blob::BlobDigest;

static FIXTURES: OnceLock<Mutex<HashMap<PathBuf, Weak<ReceiveExports>>>> = OnceLock::new();

pub(crate) struct ReceiveExports {
    pub protects: Mutex<Vec<BlobDigest>>,
    pub exports: Mutex<Vec<BlobDigest>>,
    pub paths: Mutex<Vec<PathBuf>>,
    pub obstruct_failure: AtomicBool,
    fail_at: usize,
    pub paused_exports: Mutex<BTreeSet<usize>>,
    pub paused: Notify,
    pub release: Semaphore,
}

impl ReceiveExports {
    pub fn install(root: &Path, fail_at: usize, pause_at: usize) -> Arc<Self> {
        let fixture = Arc::new(Self {
            protects: Mutex::new(Vec::new()),
            exports: Mutex::new(Vec::new()),
            paths: Mutex::new(Vec::new()),
            obstruct_failure: AtomicBool::new(false),
            fail_at,
            paused_exports: Mutex::new(BTreeSet::from([pause_at])),
            paused: Notify::new(),
            release: Semaphore::new(0),
        });
        FIXTURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .insert(root.to_owned(), Arc::downgrade(&fixture));
        fixture
    }
}

pub(crate) struct ReleaseOnDrop(pub Arc<ReceiveExports>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release.add_permits(1);
    }
}

pub(crate) fn decorate(
    path: &Path,
    inner: Arc<dyn BlobReadStorePort>,
) -> Arc<dyn BlobReadStorePort> {
    let fixture = {
        let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
        fixtures.retain(|_, fixture| fixture.strong_count() > 0);
        fixtures
            .iter()
            .find_map(|(root, fixture)| path.starts_with(root).then(|| fixture.upgrade()).flatten())
    };
    match fixture {
        Some(fixture) => Arc::new(ObservedExports { inner, fixture }),
        None => inner,
    }
}

struct ObservedExports {
    inner: Arc<dyn BlobReadStorePort>,
    fixture: Arc<ReceiveExports>,
}

#[async_trait]
impl BlobReadStorePort for ObservedExports {
    async fn protect(&self, digest: BlobDigest) -> Result<Option<BlobReceived>, BlobExchangeError> {
        self.fixture.protects.lock().unwrap().push(digest);
        self.inner.protect(digest).await
    }

    async fn read(&self, digest: BlobDigest) -> Result<Bytes, BlobExchangeError> {
        self.inner.read(digest).await
    }

    async fn export(&self, digest: BlobDigest, target: &Path) -> Result<u64, BlobExchangeError> {
        self.fixture.paths.lock().unwrap().push(target.to_owned());
        let index = {
            let mut exports = self.fixture.exports.lock().unwrap();
            let index = exports.len();
            exports.push(digest);
            index
        };
        if index == self.fixture.fail_at {
            if self.fixture.obstruct_failure.load(Ordering::SeqCst) {
                tokio::fs::remove_file(target).await.unwrap();
                tokio::fs::create_dir(target).await.unwrap();
            }
            return Err(BlobExchangeError::new(
                BlobExchangeFailure::Store,
                io::Error::from(io::ErrorKind::StorageFull),
            ));
        }
        let result = self.inner.export(digest, target).await;
        let pause = self.fixture.paused_exports.lock().unwrap().contains(&index);
        if pause {
            // 重试的真实文件已写完，但本地能力尚未交回完成权。
            self.fixture.paused.notify_one();
            self.fixture.release.acquire().await.unwrap().forget();
        }
        result
    }
}
