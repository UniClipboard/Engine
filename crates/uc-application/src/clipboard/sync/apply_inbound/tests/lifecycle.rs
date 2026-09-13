use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{oneshot, Notify};
use tokio::task::{spawn_blocking, JoinError};
use tokio::time::timeout;
use uc_core::{
    ids::{DeviceId, EntryId},
    SystemClipboardSnapshot,
};

use super::{
    fixture_input, ApplyInboundClipboardUseCase, ApplyInboundError, ApplyOutcome, AttemptGate,
    ClipboardWriteIntent, FakeAvailability, HostEventBus, InboundApplyCommonDeps, InboundCapture,
    InboundReceiveAttemptDeps, InboundWrite, MockBlobMaterializer, MockCapture, MockEntryRepo,
    MockWrite, NoopReceiveArtifactCleanup, RecordingLiveIndex, StoreOnlyPullDeps,
};
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn shutdown_releases_a_receive_waiting_for_initial_readiness() {
    let attempts = AttemptGate::empty();
    let readiness = Arc::new(ReceiveReadinessCoordinator::new());
    let receiver = ApplyInboundClipboardUseCase::store_only_pull(StoreOnlyPullDeps {
        common: InboundApplyCommonDeps {
            entry_repo: Arc::new(MockEntryRepo::new()),
            capture: Arc::new(MockCapture::new()),
            blob_materializer: Arc::new(MockBlobMaterializer::new()),
            receive_attempts: InboundReceiveAttemptDeps {
                get: attempts.clone(),
                begin: attempts.clone(),
                claim_commit: attempts.clone(),
                request_cancel: attempts.clone(),
                begin_failure: attempts.clone(),
                commit: attempts.clone(),
                clock: attempts,
            },
            receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
            receive_readiness: Arc::clone(&readiness),
            host_event_emitter: Arc::new(HostEventBus::new()),
            search_live_index: Arc::new(RecordingLiveIndex {
                calls: AtomicUsize::new(0),
            }),
            availability: Arc::new(FakeAvailability { available: true }),
            entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
        },
    });
    let (input, _) = fixture_input("waiting-initial-readiness");
    let mut receive = Box::pin(receiver.execute(input));
    assert!(timeout(Duration::from_millis(20), receive.as_mut())
        .await
        .is_err());
    let stopped = timeout(Duration::from_secs(1), receiver.shutdown()).await;
    // 失败时也解除门禁，避免测试留下持有者。
    readiness.mark_ready();
    assert!(stopped.unwrap().is_ok());
    assert!(matches!(receive.await, Err(ApplyInboundError::Stopped)));
}

#[tokio::test]
async fn receive_panic_preserves_the_same_source_for_the_caller_and_shutdown() {
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .returning(|_| panic!("private receive failure"));
    let receiver = ApplyInboundClipboardUseCase::new(
        Arc::new(repo),
        Arc::new(MockCapture::new()),
        Arc::new(MockWrite::new()),
    );
    let (input, _) = fixture_input("receive-panic");
    let error = receiver.execute(input).await.unwrap_err();
    assert!(!format!("{error:?}").contains("private"));
    let ApplyInboundError::WorkFailed(source) = error else {
        panic!("expected owned task failure")
    };
    let original = source.downcast_ref::<Arc<JoinError>>().unwrap();
    let shutdown = receiver.shutdown().await.unwrap_err();
    assert!(Arc::ptr_eq(
        original,
        shutdown.primary.downcast_ref::<Arc<JoinError>>().unwrap()
    ));
}

struct HeldWrite {
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    finished: Arc<AtomicBool>,
}

struct HeldCapture {
    entered: Arc<Notify>,
    completed: Arc<Notify>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait]
impl InboundCapture for HeldCapture {
    async fn capture(
        &self,
        entry: EntryId,
        _: DeviceId,
        _: SystemClipboardSnapshot,
    ) -> Result<Option<EntryId>> {
        let release = self.release.lock().unwrap().take().unwrap();
        let entered = Arc::clone(&self.entered);
        let completed = Arc::clone(&self.completed);
        spawn_blocking(move || {
            entered.notify_one();
            release.blocking_recv().unwrap();
            completed.notify_one();
        })
        .await?;
        Ok(Some(entry))
    }
}

#[tokio::test]
async fn cancelled_receive_waiter_does_not_abandon_the_started_capture() {
    let (release, blocked) = oneshot::channel();
    let capture = Arc::new(HeldCapture {
        entered: Arc::new(Notify::new()),
        completed: Arc::new(Notify::new()),
        release: Mutex::new(Some(blocked)),
    });
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .returning(|_| Ok(None));
    let mut write = MockWrite::new();
    let written = Arc::new(AtomicBool::new(false));
    write.expect_write().returning({
        let written = Arc::clone(&written);
        move |_, _| {
            written.store(true, Ordering::SeqCst);
            Ok(())
        }
    });
    let receiver = Arc::new(ApplyInboundClipboardUseCase::new(
        Arc::new(repo),
        capture.clone(),
        Arc::new(write),
    ));
    let (input, _) = fixture_input("cancelled-capture-waiter");
    let waiter = tokio::spawn({
        let receiver = Arc::clone(&receiver);
        async move { receiver.execute(input).await }
    });
    capture.entered.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let early = timeout(Duration::from_millis(20), receiver.shutdown()).await;
    release.send(()).unwrap();
    capture.completed.notified().await;
    assert!(early.is_err(), "shutdown must retain the started capture");
    receiver.shutdown().await.unwrap();
    assert!(written.load(Ordering::SeqCst));
}

#[async_trait]
impl InboundWrite for HeldWrite {
    async fn write(&self, _: SystemClipboardSnapshot, _: ClipboardWriteIntent) -> Result<()> {
        let release = self.release.lock().unwrap().take().unwrap();
        let finished = Arc::clone(&self.finished);
        self.entered.notify_one();
        spawn_blocking(move || {
            release.blocking_recv().unwrap();
            finished.store(true, Ordering::SeqCst);
        })
        .await?;
        Ok(())
    }
}

fn receiver(write: Arc<dyn InboundWrite>) -> ApplyInboundClipboardUseCase {
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .times(1)
        .returning(|_| Ok(None));
    let mut capture = MockCapture::new();
    capture
        .expect_capture()
        .times(1)
        .returning(|_, _, _| Ok(Some(EntryId::from("saved-entry"))));
    ApplyInboundClipboardUseCase::new(Arc::new(repo), Arc::new(capture), write)
}

#[tokio::test]
async fn capture_acknowledges_before_os_write_but_shutdown_waits_for_it() {
    let (release, blocked) = oneshot::channel();
    let write = Arc::new(HeldWrite {
        entered: Notify::new(),
        release: Mutex::new(Some(blocked)),
        finished: Arc::new(AtomicBool::new(false)),
    });
    let receiver = Arc::new(receiver(write.clone()));
    let (input, _) = fixture_input("held-system-write");
    let outcome = timeout(Duration::from_secs(1), receiver.execute(input.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
    write.entered.notified().await;
    let early = timeout(Duration::from_millis(20), receiver.shutdown()).await;
    let stopped = receiver.execute(input).await;
    let premature = write.finished.load(Ordering::SeqCst);
    release.send(()).unwrap();
    assert!(early.is_err());
    assert!(matches!(stopped, Err(ApplyInboundError::Stopped)));
    assert!(!premature);
    receiver.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
    assert!(write.finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn background_panic_is_retained_by_repeated_shutdown() {
    let mut write = MockWrite::new();
    write
        .expect_write()
        .times(1)
        .returning(|_, _| panic!("private host callback"));
    let receiver = receiver(Arc::new(write));
    let (input, _) = fixture_input("panic-system-write");
    assert!(matches!(
        receiver.execute(input).await.unwrap(),
        ApplyOutcome::Applied { .. }
    ));
    for _ in 0..2 {
        let error = receiver.shutdown().await.unwrap_err();
        assert!(error
            .primary
            .downcast_ref::<Arc<JoinError>>()
            .unwrap()
            .is_panic());
        assert!(!format!("{error:?}").contains("private"));
    }
}
