use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{oneshot, Notify};
use tokio::task::{spawn_blocking, JoinError};
use tokio::time::timeout;
use uc_core::{ids::EntryId, SystemClipboardSnapshot};

use super::{
    fixture_input, ApplyInboundClipboardUseCase, ApplyInboundError, ApplyOutcome,
    ClipboardWriteIntent, InboundWrite, MockCapture, MockEntryRepo, MockWrite,
};

struct HeldWrite {
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    finished: Arc<AtomicBool>,
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
