use std::io::{Error as IoError, ErrorKind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{oneshot, Notify};
use tokio::task::{spawn_blocking, JoinError};
use tokio::time::timeout;
use uc_core::file_transfer::FileTransferEventStorePort;
use uc_core::{FileTransferCancellationReason, FileTransferEvent};

use super::{
    entry_transfer, noop_lifecycle_deps, FileTransferApplicationError, FileTransferFacade,
    FileTransferFacadeDeps, FixedClock, InMemoryEventPublisher, InMemoryEventStore, ReceiverStore,
};

struct ClosingStore {
    inner: InMemoryEventStore,
    fail: AtomicBool,
    panics: AtomicBool,
    attempts: AtomicUsize,
    entered: Notify,
    finished: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl ClosingStore {
    fn new() -> Self {
        Self {
            inner: InMemoryEventStore::new(),
            fail: AtomicBool::new(false),
            panics: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            entered: Notify::new(),
            finished: Notify::new(),
            release: Mutex::new(None),
        }
    }
}

#[async_trait]
impl FileTransferEventStorePort for ClosingStore {
    async fn load(&self, id: &str) -> anyhow::Result<Vec<FileTransferEvent>> {
        self.inner.load(id).await
    }

    async fn append(&self, event: FileTransferEvent) -> anyhow::Result<()> {
        let closing = matches!(event, FileTransferEvent::Cancelled { .. });
        if closing {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.panics.swap(false, Ordering::SeqCst) {
                panic!("private transfer cleanup failure");
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(IoError::new(
                    ErrorKind::PermissionDenied,
                    "private transfer store failure",
                )
                .into());
            }
            if attempt == 0 {
                let release = self.release.lock().unwrap().take();
                if let Some(release) = release {
                    let native = spawn_blocking(move || release.blocking_recv().unwrap());
                    self.entered.notify_one();
                    native.await.unwrap();
                }
            }
        }
        self.inner.append(event).await?;
        if closing && self.attempts.load(Ordering::SeqCst) >= 2 {
            self.finished.notify_one();
        }
        Ok(())
    }
}

async fn facade(store: Arc<ClosingStore>) -> Arc<FileTransferFacade> {
    let receiver = Arc::new(ReceiverStore::default());
    let facade = Arc::new(FileTransferFacade::new(FileTransferFacadeDeps {
        store,
        publisher: Arc::new(InMemoryEventPublisher::new()),
        repo: receiver.clone(),
        provisional_seed: receiver.clone(),
        provisional_path: receiver.clone(),
        provisional_finalize: receiver,
        clock: Arc::new(FixedClock),
        lifecycle: noop_lifecycle_deps(),
    }));
    for id in ["first", "second"] {
        facade
            .begin_receiver_transfer(entry_transfer(id))
            .await
            .unwrap();
    }
    facade
}

#[tokio::test]
async fn close_retains_all_failures_and_can_retry_the_failed_sessions() {
    let store = Arc::new(ClosingStore::new());
    store.fail.store(true, Ordering::SeqCst);
    let facade = facade(Arc::clone(&store)).await;
    let error = facade.close().await.unwrap_err();
    assert!(!format!("{error:?}").contains("private"));
    let FileTransferApplicationError::Cleanup(report) = error else {
        panic!("expected complete cleanup report")
    };
    assert_eq!(report.additional.len(), 1);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 2);
    for source in std::iter::once(&report.primary).chain(report.additional.iter()) {
        assert!(source.chain().any(|cause| cause
            .downcast_ref::<IoError>()
            .is_some_and(|error| error.kind() == ErrorKind::PermissionDenied)));
    }
    store.fail.store(false, Ordering::SeqCst);
    facade.close().await.unwrap();
    facade.close().await.unwrap();
    assert_eq!(store.attempts.load(Ordering::SeqCst), 4);
    for id in ["first", "second"] {
        assert!(facade.active_session(id).await.is_none());
        assert_eq!(store.load(id).await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn one_panicking_session_does_not_skip_the_other_session() {
    let store = Arc::new(ClosingStore::new());
    store.panics.store(true, Ordering::SeqCst);
    store.fail.store(true, Ordering::SeqCst);
    let facade = facade(Arc::clone(&store)).await;
    let error = facade.close().await.unwrap_err();
    assert!(!format!("{error:?}").contains("private"));
    let FileTransferApplicationError::Cleanup(report) = error else {
        panic!("expected complete cleanup report")
    };
    assert_eq!(report.additional.len(), 1);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 2);
    let sources: Vec<_> = std::iter::once(&report.primary)
        .chain(report.additional.iter())
        .collect();
    assert!(sources.iter().any(|source| source.chain().any(|cause| cause
        .downcast_ref::<JoinError>()
        .is_some_and(JoinError::is_panic))));
    assert!(sources.iter().any(|source| source
        .chain()
        .any(|cause| cause.downcast_ref::<IoError>().is_some())));
    store.fail.store(false, Ordering::SeqCst);
    facade.close().await.unwrap();
}

#[tokio::test]
async fn cancelling_active_sessions_does_not_permanently_close_the_facade() {
    let store = Arc::new(ClosingStore::new());
    let facade = facade(store).await;
    facade
        .cancel_active_sessions(FileTransferCancellationReason::ConnectivityRecovery)
        .await
        .unwrap();
    facade
        .begin_receiver_transfer(entry_transfer("next"))
        .await
        .unwrap();
    facade.close().await.unwrap();
}

#[tokio::test]
async fn cancelling_the_close_waiter_still_finishes_every_session() {
    let store = Arc::new(ClosingStore::new());
    let (release, blocked) = oneshot::channel();
    *store.release.lock().unwrap() = Some(blocked);
    let facade = facade(Arc::clone(&store)).await;
    let closing = tokio::spawn({
        let facade = Arc::clone(&facade);
        async move { facade.close().await }
    });
    store.entered.notified().await;
    timeout(Duration::from_secs(1), async {
        while store.attempts.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    closing.abort();
    assert!(closing.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    timeout(Duration::from_secs(1), store.finished.notified())
        .await
        .unwrap();
    facade.close().await.unwrap();
    for id in ["first", "second"] {
        assert!(facade.active_session(id).await.is_none());
        assert_eq!(store.load(id).await.unwrap().len(), 2);
    }
}
