//! 真实接收导出边界的存储故障与完成屏障；只由测试组合根安装，不替换网络或保存流程。
use async_trait::async_trait;
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Notify, Semaphore};
use tokio_util::bytes::Bytes;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobReadStorePort, BlobReceived, ConnectivityBackend,
    ConnectivityError, PeerConnection, ProgressTerminalNotice,
};
use uc_core::ids::DeviceId;
use uc_core::ports::blob::BlobDigest;

mod progress;
pub(crate) use progress::decorate_progress_wire;

static FIXTURES: OnceLock<Mutex<HashMap<PathBuf, Weak<ReceiveExports>>>> = OnceLock::new();

pub(crate) struct ReceiveExports {
    pub protects: Mutex<Vec<BlobDigest>>,
    pub cache_misses: Mutex<Vec<BlobDigest>>,
    pub exports: Mutex<Vec<BlobDigest>>,
    pub paths: Mutex<Vec<PathBuf>>,
    pub obstruct_failure: AtomicBool,
    fail_at: usize,
    pub paused_exports: Mutex<BTreeSet<usize>>,
    pub paused: Notify,
    pub release: Semaphore,
    pub block_progress_network: AtomicBool,
    pub progress_network_blocked: Notify,
    pub pause_read: AtomicBool,
    pub read_paused: Notify,
    pub read_release: Semaphore,
    pub corrupt_read: AtomicBool,
    pub hold_terminals: AtomicBool,
    pub terminal_held: Notify,
    pub terminal_release: Semaphore,
    terminal_acks: Mutex<Vec<ProgressTerminalNotice>>,
    terminal_ack_changed: Notify,
}

impl ReceiveExports {
    pub fn install(root: &Path, fail_at: usize, pause_at: usize) -> Arc<Self> {
        let fixture = Arc::new(Self {
            protects: Mutex::new(Vec::new()),
            cache_misses: Mutex::new(Vec::new()),
            exports: Mutex::new(Vec::new()),
            paths: Mutex::new(Vec::new()),
            obstruct_failure: AtomicBool::new(false),
            fail_at,
            paused_exports: Mutex::new(BTreeSet::from([pause_at])),
            paused: Notify::new(),
            release: Semaphore::new(0),
            block_progress_network: AtomicBool::new(false),
            progress_network_blocked: Notify::new(),
            pause_read: AtomicBool::new(false),
            read_paused: Notify::new(),
            read_release: Semaphore::new(0),
            corrupt_read: AtomicBool::new(false),
            hold_terminals: AtomicBool::new(false),
            terminal_held: Notify::new(),
            terminal_release: Semaphore::new(0),
            terminal_acks: Mutex::new(Vec::new()),
            terminal_ack_changed: Notify::new(),
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
        self.0.read_release.add_permits(1);
        self.0.terminal_release.add_permits(1);
    }
}

pub(crate) fn decorate(
    path: &Path,
    inner: Arc<dyn BlobReadStorePort>,
) -> Arc<dyn BlobReadStorePort> {
    match fixture_for(path) {
        Some(fixture) => Arc::new(ObservedExports { inner, fixture }),
        None => inner,
    }
}

pub(crate) fn decorate_progress_backend(
    path: &Path,
    inner: Arc<dyn ConnectivityBackend>,
) -> Arc<dyn ConnectivityBackend> {
    match fixture_for(path) {
        Some(fixture) => Arc::new(ObservedProgressNetwork { inner, fixture }),
        None => inner,
    }
}

fn fixture_for(path: &Path) -> Option<Arc<ReceiveExports>> {
    let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
    fixtures.retain(|_, fixture| fixture.strong_count() > 0);
    fixtures
        .iter()
        .find_map(|(root, fixture)| path.starts_with(root).then(|| fixture.upgrade()).flatten())
}

struct ObservedProgressNetwork {
    inner: Arc<dyn ConnectivityBackend>,
    fixture: Arc<ReceiveExports>,
}

#[async_trait]
impl ConnectivityBackend for ObservedProgressNetwork {
    async fn connect(
        &self,
        target: DeviceId,
        address_hint: Option<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        if self.fixture.block_progress_network.load(Ordering::SeqCst) {
            self.fixture.progress_network_blocked.notify_one();
            cancel.cancelled().await;
            return Err(ConnectivityError::Stopped);
        }
        self.inner.connect(target, address_hint, cancel).await
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
        let retained = self.inner.protect(digest).await?;
        if retained.is_none() {
            self.fixture.cache_misses.lock().unwrap().push(digest);
        }
        Ok(retained)
    }

    async fn read(&self, digest: BlobDigest) -> Result<Bytes, BlobExchangeError> {
        let bytes = self.inner.read(digest).await?;
        if self.fixture.pause_read.swap(false, Ordering::SeqCst) {
            // SDK已完成真实接收；原FetchBlob尚未取得密文字节，实际AEAD尚未执行。
            self.fixture.read_paused.notify_one();
            self.fixture.read_release.acquire().await.unwrap().forget();
        }
        if self.fixture.corrupt_read.swap(false, Ordering::SeqCst) {
            // 只破坏本次交给原解密器的副本，原SDK存储及其BAO验证事实不变。
            let mut corrupted = bytes.to_vec();
            *corrupted.last_mut().unwrap() ^= 1;
            return Ok(Bytes::from(corrupted));
        }
        Ok(bytes)
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
