use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, oneshot, Notify};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{
    inbound, pull_harness, ActiveClipboardReceiverPort, ClipboardWriteCoordinator, EntryId,
    InboundActiveClipboardState, PullClientSpy, StoreSpy, StubOrigin, SystemClipboardPort,
    SystemClipboardSnapshot,
};

struct Receiver(broadcast::Sender<InboundActiveClipboardState>);

#[async_trait]
impl ActiveClipboardReceiverPort for Receiver {
    fn subscribe(&self) -> broadcast::Receiver<InboundActiveClipboardState> {
        self.0.subscribe()
    }
}

struct HeldClipboard {
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl SystemClipboardPort for HeldClipboard {
    fn read_snapshot(&self) -> anyhow::Result<SystemClipboardSnapshot> {
        panic!("unexpected read");
    }

    fn write_snapshot(&self, _snapshot: SystemClipboardSnapshot) -> anyhow::Result<()> {
        let release = self.release.lock().unwrap().take().unwrap();
        self.entered.notify_one();
        release.blocking_recv().unwrap();
        Ok(())
    }
}

#[tokio::test]
async fn stopped_receiver_waits_for_the_started_os_write_and_register_update() {
    let entry_id = EntryId::from("entry-pulled");
    let store = Arc::new(StoreSpy {
        entry_id: entry_id.clone(),
        seen_envelope: Mutex::new(None),
    });
    let mut harness = pull_harness(Some(PullClientSpy::new(Ok(vec![1]))), Some(store), entry_id);
    let (release, blocked) = oneshot::channel();
    let clipboard = Arc::new(HeldClipboard {
        entered: Notify::new(),
        release: Mutex::new(Some(blocked)),
    });
    harness.uc.coordinator = Arc::new(ClipboardWriteCoordinator::new(
        clipboard.clone(),
        Arc::new(StubOrigin),
    ));
    let (send, _) = broadcast::channel(4);
    harness.uc.receiver = Arc::new(Receiver(send.clone()));
    let cancel = CancellationToken::new();
    let mut worker = tokio::spawn(Arc::new(harness.uc).run(cancel.clone()));
    timeout(Duration::from_secs(1), async {
        while send.receiver_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    send.send(inbound("blake3v1:aa", 1_000, "dev-x")).unwrap();
    clipboard.entered.notified().await;
    cancel.cancel();
    let premature = timeout(Duration::from_millis(20), &mut worker).await;
    let before_release = harness.advance.calls.load(Ordering::SeqCst);
    release.send(()).unwrap();
    assert!(premature.is_err());
    assert_eq!(before_release, 0);
    worker.await.unwrap();
    assert_eq!(harness.advance.calls.load(Ordering::SeqCst), 1);
}
