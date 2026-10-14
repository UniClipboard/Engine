//! 真依赖屏障：先让 Iroh 导入文件 A，再阻塞完整发布动作的返回。
//! 仅测试组合根使用，按临时资料根隔离，不增加生产或稳定接口。
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Notify, Semaphore};
use tokio_util::bytes::Bytes;
use uc_application::deps::{
    ActivePullPreparationError, ActivePullPreparationPort, ActivePullPreparationStop,
};
use uc_core::ports::blob::{
    BlobDigest, BlobError, BlobProgressSink, BlobTicket, BlobTransferPort, TagReason,
};

static FIXTURES: OnceLock<Mutex<HashMap<PathBuf, Weak<PublicationFixture>>>> = OnceLock::new();
fn fixtures() -> &'static Mutex<HashMap<PathBuf, Weak<PublicationFixture>>> {
    FIXTURES.get_or_init(Mutex::default)
}
fn lookup(path: &Path) -> Option<Arc<PublicationFixture>> {
    let mut fixtures = fixtures().lock().unwrap();
    fixtures.retain(|_, fixture| fixture.strong_count() > 0);
    fixtures
        .iter()
        .find_map(|(root, fixture)| path.starts_with(root).then(|| fixture.upgrade()).flatten())
}
pub(crate) struct PublicationFixture {
    pub armed: AtomicBool,
    pub publications: AtomicUsize,
    pub started: Notify,
    pub release: Semaphore,
    pub completed: Notify,
    pub finished: AtomicBool,
    pub cancelled: AtomicBool,
    pub stop: Mutex<Option<ActivePullPreparationStop>>,
    pub real_imports: AtomicUsize,
}
impl PublicationFixture {
    pub fn install(root: &Path) -> Arc<Self> {
        let fixture = Arc::new(Self {
            armed: AtomicBool::new(false),
            publications: AtomicUsize::new(0),
            started: Notify::new(),
            release: Semaphore::new(0),
            completed: Notify::new(),
            finished: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            stop: Mutex::new(None),
            real_imports: AtomicUsize::new(0),
        });
        fixtures()
            .lock()
            .unwrap()
            .insert(root.to_owned(), Arc::downgrade(&fixture));
        fixture
    }
}
pub(crate) struct ReleaseOnDrop(pub Arc<PublicationFixture>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release.add_permits(16);
    }
}

pub(crate) fn decorate_blob(
    path: &Path,
    inner: Arc<dyn BlobTransferPort>,
) -> Arc<dyn BlobTransferPort> {
    match lookup(path) {
        Some(fixture) => Arc::new(GatedPublication { inner, fixture }),
        None => inner,
    }
}
pub(crate) fn decorate_preparation(
    path: &Path,
    inner: Arc<dyn ActivePullPreparationPort>,
) -> Arc<dyn ActivePullPreparationPort> {
    match lookup(path) {
        Some(fixture) => Arc::new(ObservedPreparation { inner, fixture }),
        None => inner,
    }
}
struct ObservedPreparation {
    inner: Arc<dyn ActivePullPreparationPort>,
    fixture: Arc<PublicationFixture>,
}
#[async_trait]
impl ActivePullPreparationPort for ObservedPreparation {
    async fn prepare(
        &self,
        hash: &str,
        stop: &ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        *self.fixture.stop.lock().unwrap() = Some(stop.clone());
        let result = self.inner.prepare(hash, stop).await;
        self.fixture.cancelled.store(
            matches!(&result, Err(ActivePullPreparationError::Cancelled(_))),
            Ordering::SeqCst,
        );
        self.fixture.finished.store(true, Ordering::SeqCst);
        self.fixture.completed.notify_one();
        result
    }
}
struct GatedPublication {
    inner: Arc<dyn BlobTransferPort>,
    fixture: Arc<PublicationFixture>,
}
#[async_trait]
impl BlobTransferPort for GatedPublication {
    async fn publish(&self, bytes: Bytes, reason: TagReason) -> Result<BlobDigest, BlobError> {
        self.inner.publish(bytes, reason).await
    }
    async fn publish_path(&self, path: &Path, reason: TagReason) -> Result<BlobDigest, BlobError> {
        let armed = self.fixture.armed.load(Ordering::SeqCst);
        let index = if armed {
            self.fixture.publications.fetch_add(1, Ordering::SeqCst)
        } else {
            usize::MAX
        };
        let digest = self.inner.publish_path(path, reason).await?;
        if armed {
            assert!(
                self.inner.has(&digest).await.unwrap(),
                "真实导入必须已经可读，不能以 fake 发布替代"
            );
            self.fixture.real_imports.fetch_add(1, Ordering::SeqCst);
            if index == 0 {
                self.fixture.started.notify_one();
                self.fixture.release.acquire().await.unwrap().forget();
            }
        }
        Ok(digest)
    }
    async fn issue_ticket(&self, digest: &BlobDigest) -> Result<BlobTicket, BlobError> {
        self.inner.issue_ticket(digest).await
    }
    async fn fetch(
        &self,
        ticket: &BlobTicket,
        progress: Option<&dyn BlobProgressSink>,
    ) -> Result<Bytes, BlobError> {
        self.inner.fetch(ticket, progress).await
    }
    async fn fetch_to_path(
        &self,
        ticket: &BlobTicket,
        path: &Path,
        progress: Option<&dyn BlobProgressSink>,
    ) -> Result<BlobDigest, BlobError> {
        self.inner.fetch_to_path(ticket, path, progress).await
    }
    async fn shutdown_inflight_fetch(&self, ticket: &BlobTicket) -> Result<(), BlobError> {
        self.inner.shutdown_inflight_fetch(ticket).await
    }
    async fn has(&self, digest: &BlobDigest) -> Result<bool, BlobError> {
        self.inner.has(digest).await
    }
    async fn tag(&self, digest: &BlobDigest, reason: TagReason) -> Result<(), BlobError> {
        self.inner.tag(digest, reason).await
    }
    async fn untag(&self, reason: TagReason) -> Result<(), BlobError> {
        self.inner.untag(reason).await
    }
    fn digest_of(&self, ticket: &BlobTicket) -> Result<BlobDigest, BlobError> {
        self.inner.digest_of(ticket)
    }
}
