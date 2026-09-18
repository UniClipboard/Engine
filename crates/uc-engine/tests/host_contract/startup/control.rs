use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::timeout;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, EngineErrorCategory, EngineEvent, EngineState,
    HistoryEntryInput, Operation, OperationResult, SecretString, SendTextInput, StartupLifecycle,
    StartupProgress, StartupState,
};

use super::super::lease::{find_lease, open_lease};
use super::{host, HeldSecureStorage, MemorySecureStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pause_during_real_startup_finishes_before_handoff_and_preserves_saved_content() {
    pause_during_startup(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_startup_pause_keeps_admission_closed_until_cleanup_is_confirmed() {
    pause_during_startup(true).await;
}

async fn pause_during_startup(expired: bool) {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("startup lifecycle test".into()),
            passphrase: SecretString::new("startup-lifecycle-passphrase"),
            passphrase_confirmation: SecretString::new("startup-lifecycle-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(saved) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "confirmed before controlled startup".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved entry")
    };
    let lease = find_lease(root.path()).unwrap();
    engine.shutdown_until_complete().await.unwrap();
    drop(engine);
    drop(events);

    let (release, blocked) = mpsc::channel();
    let entered = Arc::new(Notify::new());
    let blocked_host = host(
        root.path(),
        Box::new(HeldSecureStorage {
            storage,
            entered: entered.clone(),
            release: Arc::new(Mutex::new(Some(blocked))),
        }),
    );
    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, control) = StartupLifecycle::channel();
    let starting = tokio::spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        blocked_host,
        progress_input,
        lifecycle_input,
    ));
    timeout(Duration::from_secs(15), entered.notified())
        .await
        .unwrap();
    let mut pause = Box::pin(async {
        if expired {
            control.suspend_with_deadline(Duration::ZERO).await
        } else {
            control.suspend().await
        }
    });
    let early = timeout(Duration::from_millis(100), pause.as_mut()).await;
    release.send(()).unwrap();
    if expired {
        assert_eq!(
            early.unwrap().unwrap_err().category(),
            EngineErrorCategory::DeadlineExceeded
        );
    } else {
        assert!(early.is_err());
    }
    let (engine, mut events) = timeout(Duration::from_secs(20), starting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(progress.snapshot().state, StartupState::Ready);
    if expired {
        assert!(matches!(
            engine.lifecycle_state().await,
            EngineState::Quiesced | EngineState::Suspended
        ));
    } else {
        pause.await.unwrap();
        assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    }
    assert!(engine.execute(Operation::ListDevices).await.is_err());
    control.suspend().await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    let competing = open_lease(&lease);
    competing.try_lock().unwrap();
    drop(competing);
    while let Ok(Some(event)) = timeout(Duration::ZERO, events.next()).await {
        assert!(!matches!(
            event,
            EngineEvent::StateChanged {
                state: EngineState::Running
            }
        ));
    }

    control.resume().await.unwrap();
    let OperationResult::HistoryEntry(entry) = engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: saved.entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved content")
    };
    assert_eq!(entry.content, "confirmed before controlled startup");
    engine.suspend().await.unwrap();
    control.resume().await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
    assert_eq!(
        control.resume().await.unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
}
