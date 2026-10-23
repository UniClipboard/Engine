//! 稳定 Engine 双流验收的真实 SDK 读取屏障；不替换网络、校验、结算或业务结果。
use async_trait::async_trait;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangePort, BlobReceived, BlobValidatedProgress, ConnectivityError,
    PeerReceiveStream, StreamLease,
};
use uc_core::ports::blob::BlobDigest;

static FIXTURES: OnceLock<Mutex<Vec<(PathBuf, Weak<ReceiveStreams>)>>> = OnceLock::new();
const VERIFIED_PREFIX: u64 = 64 * 1024;

pub(crate) struct ReceiveStream {
    pub digest: BlobDigest,
    pub stream_id: u64,
    pub validated: AtomicU64,
    pub total: AtomicU64,
    pub paused: AtomicBool,
    pub stopped: AtomicBool,
    pub finished: AtomicBool,
    pub injected_failure: AtomicBool,
    released: AtomicBool,
    reader: Mutex<Option<Waker>>,
}
impl ReceiveStream {
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        let reader = self.reader.lock().unwrap().take();
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

pub(crate) struct ReceiveStreams {
    gate_receives: AtomicBool,
    failure: Mutex<Option<(BlobDigest, io::ErrorKind)>>,
    receives: Mutex<Vec<Arc<ReceiveStream>>>,
    pub provides: Mutex<Vec<u64>>,
}
impl ReceiveStreams {
    pub fn install(root: &Path, gate_receives: bool) -> Arc<Self> {
        let fixture = Arc::new(Self {
            gate_receives: AtomicBool::new(gate_receives),
            failure: Mutex::new(None),
            receives: Mutex::new(Vec::new()),
            provides: Mutex::new(Vec::new()),
        });
        FIXTURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .push((root.to_owned(), Arc::downgrade(&fixture)));
        fixture
    }
    pub fn arm(&self) {
        self.gate_receives.store(true, Ordering::SeqCst);
    }
    pub fn fail_other_content_once(&self, excluded: BlobDigest, kind: io::ErrorKind) {
        *self.failure.lock().unwrap() = Some((excluded, kind));
    }
    pub fn receives(&self) -> Vec<Arc<ReceiveStream>> {
        self.receives.lock().unwrap().clone()
    }
    pub fn release_all(&self) {
        for receive in self.receives() {
            receive.release();
        }
    }
}

pub(crate) struct ReleaseStreamsOnDrop(pub Arc<ReceiveStreams>);
impl Drop for ReleaseStreamsOnDrop {
    fn drop(&mut self) {
        self.0.release_all();
    }
}

pub(crate) fn decorate(path: &Path, inner: Arc<dyn BlobExchangePort>) -> Arc<dyn BlobExchangePort> {
    let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
    fixtures.retain(|(_, fixture)| fixture.strong_count() > 0);
    let fixture = fixtures
        .iter()
        .find_map(|(root, fixture)| path.starts_with(root).then(|| fixture.upgrade()).flatten());
    match fixture {
        Some(fixture) => Arc::new(ObservedExchange { inner, fixture }),
        None => inner,
    }
}

struct ObservedExchange {
    inner: Arc<dyn BlobExchangePort>,
    fixture: Arc<ReceiveStreams>,
}
#[async_trait]
impl BlobExchangePort for ObservedExchange {
    fn connection_failure(&self, source: ConnectivityError) -> BlobExchangeError {
        self.inner.connection_failure(source)
    }
    async fn receive(
        &self,
        mut stream: StreamLease,
        digest: BlobDigest,
        cancel: CancellationToken,
        progress: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        let observation = Arc::new(ReceiveStream {
            digest,
            stream_id: stream.receive.id(),
            validated: AtomicU64::new(0),
            total: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            injected_failure: AtomicBool::new(false),
            released: AtomicBool::new(!self.fixture.gate_receives.load(Ordering::SeqCst)),
            reader: Mutex::new(None),
        });
        self.fixture
            .receives
            .lock()
            .unwrap()
            .push(observation.clone());
        stream.receive = Box::new(ObservedReader {
            inner: stream.receive,
            observation: observation.clone(),
            fixture: self.fixture.clone(),
        });
        let progress = Arc::new(ObservedProgress {
            inner: progress,
            observation: observation.clone(),
        });
        let result = self.inner.receive(stream, digest, cancel, progress).await;
        observation.finished.store(true, Ordering::SeqCst);
        result
    }
    async fn provide(
        &self,
        stream: StreamLease,
        request_deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        self.fixture.provides.lock().unwrap().push(stream.send.id());
        self.inner.provide(stream, request_deadline, cancel).await
    }
}

struct ObservedProgress {
    inner: Arc<dyn BlobValidatedProgress>,
    observation: Arc<ReceiveStream>,
}
impl BlobValidatedProgress for ObservedProgress {
    fn validated_through(&self, end: u64, total: u64) -> Result<(), BlobExchangeError> {
        self.inner.validated_through(end, total)?;
        self.observation.total.store(total, Ordering::SeqCst);
        self.observation.validated.store(end, Ordering::SeqCst);
        Ok(())
    }
}

struct ObservedReader {
    fixture: Arc<ReceiveStreams>,
    inner: Box<dyn PeerReceiveStream>,
    observation: Arc<ReceiveStream>,
}
impl AsyncRead for ObservedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.observation.validated.load(Ordering::SeqCst) >= VERIFIED_PREFIX {
            let failure = {
                let mut pending = self.fixture.failure.lock().unwrap();
                if pending
                    .as_ref()
                    .is_some_and(|(excluded, _)| excluded != &self.observation.digest)
                {
                    pending.take().map(|(_, kind)| kind)
                } else {
                    None
                }
            };
            if let Some(kind) = failure {
                // 在真实 SDK 已校验前缀后模拟一次具体 IO 故障；分类和恢复仍由生产路径决定。
                self.observation
                    .injected_failure
                    .store(true, Ordering::SeqCst);
                return Poll::Ready(Err(io::Error::from(kind)));
            }
        }
        if self.observation.validated.load(Ordering::SeqCst) >= VERIFIED_PREFIX
            && !self.observation.released.load(Ordering::SeqCst)
        {
            *self.observation.reader.lock().unwrap() = Some(cx.waker().clone());
            if !self.observation.released.load(Ordering::SeqCst) {
                self.observation.paused.store(true, Ordering::SeqCst);
                return Poll::Pending;
            }
        }
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}
impl PeerReceiveStream for ObservedReader {
    fn id(&self) -> u64 {
        self.inner.id()
    }
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        let result = self.inner.stop(code);
        self.observation.stopped.store(true, Ordering::SeqCst);
        result
    }
}
