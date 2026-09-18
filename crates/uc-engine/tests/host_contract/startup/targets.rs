use std::future::{poll_fn, Future};
use std::sync::{mpsc, Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::timeout;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, EngineErrorCategory, EngineEvent, EngineState,
    HistoryEntryInput, Operation, OperationResult, SecretString, SendTextInput,
};

use super::super::lease::{find_lease, open_lease};
use super::{host, HeldSecureStorage, MemorySecureStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_during_real_resume_releases_the_profile_without_publishing_stale_running() {
    let root = tempfile::tempdir().unwrap();
    let entered = Arc::new(Notify::new());
    let release_slot = Arc::new(Mutex::new(None));
    let (engine, mut events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(
            root.path(),
            Box::new(HeldSecureStorage {
                storage: MemorySecureStorage::default(),
                entered: entered.clone(),
                release: release_slot.clone(),
            }),
        ),
    )
    .await
    .unwrap();
    let engine = Arc::new(engine);
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("queued lifecycle test".into()),
            passphrase: SecretString::new("queued-lifecycle-passphrase"),
            passphrase_confirmation: SecretString::new("queued-lifecycle-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(saved) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "confirmed before rapid lifecycle changes".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved entry");
    };
    let lease = find_lease(root.path()).unwrap();
    engine.suspend().await.unwrap();
    while let Ok(Some(_)) = timeout(Duration::ZERO, events.next()).await {}

    let (release, wait) = mpsc::channel();
    *release_slot.lock().unwrap() = Some(wait);
    let resuming = tokio::spawn({
        let engine = engine.clone();
        async move { engine.resume().await }
    });
    timeout(Duration::from_secs(15), entered.notified())
        .await
        .unwrap();
    let mut pause = Box::pin(engine.suspend());
    poll_fn(|context| {
        assert!(pause.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let admission = timeout(
        Duration::from_millis(100),
        engine.execute(Operation::ListDevices),
    )
    .await;
    release.send(()).unwrap();
    assert_eq!(
        admission.unwrap().unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    assert_eq!(
        resuming.await.unwrap().unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    timeout(Duration::from_secs(15), pause)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    let other_owner = open_lease(&lease);
    other_owner.try_lock().unwrap();
    drop(other_owner);
    while let Ok(Some(event)) = timeout(Duration::ZERO, events.next()).await {
        assert!(!matches!(
            event,
            EngineEvent::StateChanged {
                state: EngineState::Running
            }
        ));
    }

    engine.resume().await.unwrap();
    let OperationResult::HistoryEntry(entry) = engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: saved.entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved content after the final resume");
    };
    assert_eq!(entry.content, "confirmed before rapid lifecycle changes");
    engine.shutdown_until_complete().await.unwrap();
}
