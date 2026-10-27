//! 原生目录探测完成后的真实依赖屏障；仅隔离进程测试安装。
use async_trait::async_trait;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Notify, Semaphore};
use uc_core::ports::atomic_publish::{AtomicPublishPort, PublishError};

static FIXTURES: OnceLock<Mutex<HashMap<PathBuf, Weak<DirectoryProbe>>>> = OnceLock::new();

pub(crate) struct DirectoryProbe {
    pub entered: Notify,
    pub supported: AtomicBool,
    pub area: Mutex<Option<PathBuf>>,
    release: Semaphore,
    publication_failure: bool,
    pub published: AtomicUsize,
    pub withdrawals: AtomicUsize,
    pub withdrawn: AtomicUsize,
}
impl DirectoryProbe {
    pub fn install(destination: &Path, publication_failure: bool) -> Arc<Self> {
        let fixture = Arc::new(Self {
            entered: Notify::new(),
            supported: AtomicBool::new(false),
            area: Mutex::new(None),
            release: Semaphore::new(0),
            publication_failure,
            published: AtomicUsize::new(0),
            withdrawals: AtomicUsize::new(0),
            withdrawn: AtomicUsize::new(0),
        });
        FIXTURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .insert(destination.to_owned(), Arc::downgrade(&fixture));
        fixture
    }
}

pub(crate) fn decorate(inner: Arc<dyn AtomicPublishPort>) -> Arc<dyn AtomicPublishPort> {
    let installed = {
        let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
        fixtures.retain(|_, fixture| fixture.strong_count() > 0);
        !fixtures.is_empty()
    };
    if installed {
        Arc::new(GatedProbe { inner })
    } else {
        inner
    }
}

struct GatedProbe {
    inner: Arc<dyn AtomicPublishPort>,
}
#[async_trait]
impl AtomicPublishPort for GatedProbe {
    async fn publish_no_replace(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), PublishError> {
        // 前两根仍由真实文件系统发布；第三根失败，第二根实际撤回，首根撤回失败。
        let fixture = {
            let fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
            fixtures.iter().find_map(|(root, fixture)| {
                let fixture = fixture.upgrade()?;
                (fixture.publication_failure
                    && (source.parent() == Some(root.as_path())
                        || destination.parent() == Some(root.as_path())))
                .then_some((root.clone(), fixture))
            })
        };
        if let Some((root, fixture)) = fixture {
            if source.parent() == Some(root.as_path()) {
                fixture.withdrawals.fetch_add(1, Ordering::SeqCst);
                if source
                    .file_name()
                    .is_some_and(|name| name == "directory-other")
                {
                    self.inner.publish_no_replace(source, destination).await?;
                    fixture.withdrawn.fetch_add(1, Ordering::SeqCst);
                    return Ok(());
                }
                return Err(PublishError::Io(Box::new(io::Error::other(
                    "injected directory withdrawal failure",
                ))));
            }
            if fixture.published.load(Ordering::SeqCst) >= 2 {
                return Err(PublishError::Io(Box::new(io::Error::other(
                    "injected later directory publication failure",
                ))));
            }
            self.inner.publish_no_replace(source, destination).await?;
            fixture.published.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        self.inner.publish_no_replace(source, destination).await
    }
    async fn publish_into_free_name(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), PublishError> {
        self.inner.publish_into_free_name(source, destination).await
    }
    async fn supports_no_replace(&self, probe_dir: &Path) -> bool {
        let supported = self.inner.supports_no_replace(probe_dir).await;
        let fixture = {
            let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
            fixtures.retain(|_, fixture| fixture.strong_count() > 0);
            fixtures.iter().find_map(|(root, fixture)| {
                probe_dir
                    .starts_with(root)
                    .then(|| fixture.upgrade())
                    .flatten()
            })
        };
        if let Some(fixture) = fixture {
            fixture.supported.store(supported, Ordering::SeqCst);
            *fixture.area.lock().unwrap() = Some(probe_dir.to_owned());
            fixture.entered.notify_one();
            if !fixture.publication_failure {
                fixture.release.acquire().await.unwrap().forget();
            }
        }
        supported
    }
}
