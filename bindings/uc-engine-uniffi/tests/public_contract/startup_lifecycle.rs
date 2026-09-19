use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use super::lifecycle_targets::ReadGate;
use super::{
    engine_test_guard, lock, BindingConfig, BindingEngineState, BindingError, BindingErrorCategory,
    MemoryHost, MobileEngine, MobileStartupLifecycle, ENGINE_SHUTDOWN_DEADLINE_MS,
};

#[test]
fn mobile_startup_accepts_a_pause_before_the_engine_is_returned() {
    let _guard = engine_test_guard();
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(MemoryHost::new(root.path()));
    let config = BindingConfig {
        app_version: "1.2.3".into(),
        profile_id: "mobile-startup-lifecycle".into(),
    };
    let initial = MobileEngine::start(config.clone(), host.clone()).unwrap();
    initial
        .create_space(
            Some("mobile startup lifecycle".into()),
            "correct horse battery staple".into(),
        )
        .unwrap();
    initial.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();

    let (entered, waiting) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    *lock(&host.secure_read_gate) = Some(ReadGate {
        key_prefix: "kek:v1:",
        matches_before_wait: 0,
        entered,
        release: proceed,
    });
    let lifecycle = MobileStartupLifecycle::new();
    let starting = thread::spawn({
        let lifecycle = lifecycle.clone();
        let host = host.clone();
        let config = config.clone();
        move || MobileEngine::start_with_lifecycle(config, host, lifecycle)
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();

    let started_at = Instant::now();
    assert!(matches!(
        lifecycle.suspend_with_deadline(0),
        Err(BindingError::Engine {
            category: BindingErrorCategory::DeadlineExceeded,
            ..
        })
    ));
    assert!(started_at.elapsed() < Duration::from_secs(1));
    release.send(()).unwrap();

    let engine = starting.join().unwrap().unwrap();
    assert!(matches!(
        engine.lifecycle_state().unwrap(),
        BindingEngineState::Quiesced | BindingEngineState::Suspended
    ));
    assert!(engine.list_devices().is_err());
    lifecycle
        .suspend_with_deadline(ENGINE_SHUTDOWN_DEADLINE_MS)
        .unwrap();
    assert_eq!(
        engine.lifecycle_state().unwrap(),
        BindingEngineState::Suspended
    );
    lifecycle.resume().unwrap();
    assert!(engine.list_devices().is_ok());

    let another_root = tempfile::tempdir().unwrap();
    let reused = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "reused-startup-lifecycle".into(),
        },
        Arc::new(MemoryHost::new(another_root.path())),
        lifecycle.clone(),
    );
    assert!(matches!(
        reused,
        Err(BindingError::Engine {
            category: BindingErrorCategory::InvalidState,
            ..
        })
    ));
    engine.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();
    assert!(matches!(
        lifecycle.resume(),
        Err(BindingError::Engine {
            category: BindingErrorCategory::InvalidState,
            ..
        })
    ));
}
