//! 稳定 Engine 双流验收的真实 SDK 读取屏障；不替换网络、校验、结算或业务结果。
use async_trait::async_trait;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::{error::Elapsed, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangeFailure, BlobExchangePort, BlobReceived, BlobValidatedProgress,
    ConnectivityError, PeerReceiveStream, StreamLease,
};
use uc_core::error_class::ErrorClass;
use uc_core::ports::blob::BlobDigest;

#[path = "receive_streams/presence.rs"]
mod presence;
pub(crate) use presence::{decorate_presence, PresenceFault};

static FIXTURES: OnceLock<Mutex<Vec<(PathBuf, Weak<ReceiveStreams>)>>> = OnceLock::new();
const VERIFIED_PREFIX: u64 = 64 * 1024;

pub(crate) struct ResponseObservation {
    pub entered_at: Instant,
    pub logical_deadline: Instant,
    pub response_timeout: Duration,
    pub held_before_first_read: bool,
    pub first_read_at: Mutex<Option<Instant>>,
    pub stopped_at: Mutex<Option<Instant>>,
    pub stop_succeeded: Mutex<Option<bool>>,
    pub finished_at: Mutex<Option<Instant>>,
    pub error_class: Mutex<Option<&'static str>>,
    pub timeout_elapsed: AtomicBool,
    pub additional_count: AtomicUsize,
    pub succeeded: AtomicBool,
}

pub(crate) struct ReceiveStream {
    pub response: ResponseObservation,
    pub digest: BlobDigest,
    pub stream_id: u64,
    pub validated: AtomicU64,
    pub read: AtomicU64,
    pub total: AtomicU64,
    pub paused: AtomicBool,
    pub stopped: AtomicBool,
    pub finished: AtomicBool,
    pub injected_failure: AtomicBool,
    pub failure: Mutex<Option<(BlobExchangeFailure, Option<io::ErrorKind>)>>,
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
    pub presence: PresenceFault,
    gate_receives: AtomicBool,
    first_read_once: AtomicBool,
    header_digest_once: Mutex<Option<BlobDigest>>,
    body_digest: Mutex<Option<BlobDigest>>,
    failure: Mutex<Option<(BlobDigest, io::ErrorKind)>>,
    receives: Mutex<Vec<Arc<ReceiveStream>>>,
    pub provides: Mutex<Vec<u64>>,
}
impl ReceiveStreams {
    pub fn install(root: &Path, gate_receives: bool) -> Arc<Self> {
        let fixture = Arc::new(Self {
            presence: PresenceFault::default(),
            gate_receives: AtomicBool::new(gate_receives),
            first_read_once: AtomicBool::new(false),
            header_digest_once: Mutex::new(None),
            body_digest: Mutex::new(None),
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
    pub fn hold_first_read_once(&self) {
        assert!(!self.first_read_once.swap(true, Ordering::SeqCst));
    }
    pub fn hold_digest_header_once(&self, digest: BlobDigest) {
        assert!(self
            .header_digest_once
            .lock()
            .unwrap()
            .replace(digest)
            .is_none());
    }
    pub fn hold_digest_body(&self, digest: BlobDigest) {
        assert!(self.body_digest.lock().unwrap().replace(digest).is_none());
    }
    pub fn fail_other_content_once(&self, excluded: BlobDigest, kind: io::ErrorKind) {
        *self.failure.lock().unwrap() = Some((excluded, kind));
    }
    pub fn receives(&self) -> Vec<Arc<ReceiveStream>> {
        self.receives.lock().unwrap().clone()
    }
    pub fn release_all(&self) {
        self.header_digest_once.lock().unwrap().take();
        self.body_digest.lock().unwrap().take();
        for receive in self.receives() {
            receive.release();
        }
    }
}

pub(crate) struct ReleaseStreamsOnDrop(pub Arc<ReceiveStreams>);
impl Drop for ReleaseStreamsOnDrop {
    fn drop(&mut self) {
        self.0.presence.disarm();
        self.0.release_all();
    }
}

fn registered_fixture(path: &Path) -> Option<Arc<ReceiveStreams>> {
    let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
    fixtures.retain(|(_, fixture)| fixture.strong_count() > 0);
    fixtures
        .iter()
        .find_map(|(root, fixture)| path.starts_with(root).then(|| fixture.upgrade()).flatten())
}

pub(crate) fn decorate(path: &Path, inner: Arc<dyn BlobExchangePort>) -> Arc<dyn BlobExchangePort> {
    match registered_fixture(path) {
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
        logical_deadline: Instant,
        response_timeout: Duration,
        digest: BlobDigest,
        cancel: CancellationToken,
        progress: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        let selected_header = {
            let mut selected = self.fixture.header_digest_once.lock().unwrap();
            if selected.as_ref() == Some(&digest) {
                selected.take().is_some()
            } else {
                false
            }
        };
        let initial_hold =
            self.fixture.first_read_once.swap(false, Ordering::SeqCst) || selected_header;
        let selected_body = self.fixture.body_digest.lock().unwrap().as_ref() == Some(&digest);
        let observation = Arc::new(ReceiveStream {
            response: ResponseObservation {
                entered_at: Instant::now(),
                logical_deadline,
                response_timeout,
                held_before_first_read: initial_hold,
                first_read_at: Mutex::new(None),
                stopped_at: Mutex::new(None),
                stop_succeeded: Mutex::new(None),
                finished_at: Mutex::new(None),
                error_class: Mutex::new(None),
                timeout_elapsed: AtomicBool::new(false),
                additional_count: AtomicUsize::new(0),
                succeeded: AtomicBool::new(false),
            },
            digest,
            stream_id: stream.receive.id(),
            validated: AtomicU64::new(0),
            read: AtomicU64::new(0),
            total: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            injected_failure: AtomicBool::new(false),
            failure: Mutex::new(None),
            released: AtomicBool::new(
                !initial_hold
                    && !selected_body
                    && !self.fixture.gate_receives.load(Ordering::SeqCst),
            ),
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
        let result = self
            .inner
            .receive(
                stream,
                logical_deadline,
                response_timeout,
                digest,
                cancel,
                progress,
            )
            .await;
        observation
            .response
            .succeeded
            .store(result.is_ok(), Ordering::SeqCst);
        if let Err(error) = &result {
            *observation.response.error_class.lock().unwrap() = Some(error.class());
            observation.response.timeout_elapsed.store(
                error.source.chain().any(|source| source.is::<Elapsed>()),
                Ordering::SeqCst,
            );
            observation
                .response
                .additional_count
                .store(error.additional.len(), Ordering::SeqCst);
            let io_kind = error
                .source
                .chain()
                .find_map(|source| source.downcast_ref::<io::Error>().map(io::Error::kind));
            *observation.failure.lock().unwrap() = Some((error.kind, io_kind));
        }
        *observation.response.finished_at.lock().unwrap() = Some(Instant::now());
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
        self.observation
            .response
            .first_read_at
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
        if self.observation.response.held_before_first_read
            && !self.observation.released.load(Ordering::SeqCst)
        {
            *self.observation.reader.lock().unwrap() = Some(cx.waker().clone());
            if !self.observation.released.load(Ordering::SeqCst) {
                self.observation.paused.store(true, Ordering::SeqCst);
                return Poll::Pending;
            }
        }
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
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        self.observation
            .read
            .fetch_add((buffer.filled().len() - before) as u64, Ordering::SeqCst);
        result
    }
}
impl PeerReceiveStream for ObservedReader {
    fn id(&self) -> u64 {
        self.inner.id()
    }
    fn stop(&mut self, code: u64) -> Result<(), ConnectivityError> {
        let result = self.inner.stop(code);
        *self.observation.response.stop_succeeded.lock().unwrap() = Some(result.is_ok());
        *self.observation.response.stopped_at.lock().unwrap() = Some(Instant::now());
        self.observation.stopped.store(true, Ordering::SeqCst);
        result
    }
}
