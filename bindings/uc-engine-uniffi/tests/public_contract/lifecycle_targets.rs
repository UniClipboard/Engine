use std::sync::{mpsc, Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    engine_test_guard, lock, BindingConfig, BindingEngineState, BindingError, BindingErrorCategory,
    MemoryHost, MobileEngine, ENGINE_SHUTDOWN_DEADLINE_MS,
};

pub(super) struct ReadGate {
    pub(super) key_prefix: &'static str,
    pub(super) matches_before_wait: usize,
    pub(super) entered: mpsc::Sender<()>,
    pub(super) release: mpsc::Receiver<()>,
}

impl ReadGate {
    pub(super) fn matches(&mut self, key: &str) -> bool {
        if !key.starts_with(self.key_prefix) {
            return false;
        }
        if self.matches_before_wait > 0 {
            self.matches_before_wait -= 1;
            return false;
        }
        true
    }

    pub(super) fn wait(self) {
        self.entered.send(()).unwrap();
        let _ = self.release.recv_timeout(Duration::from_secs(15));
    }
}

#[test]
fn a_pause_reaches_the_engine_while_the_previous_mobile_resume_is_waiting() {
    // The first KEK read opens the encrypted profile-secret file. The second
    // restores the space session used by the previous mobile runtime.
    pause_during_key_read("kek:v1:", 1);
}

#[test]
fn a_pause_reaches_the_engine_while_the_profile_vault_key_is_waiting() {
    pause_during_key_read("kek:v1:", 0);
}

fn pause_during_key_read(key_prefix: &'static str, matches_before_wait: usize) {
    let _guard = engine_test_guard();
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "mobile-targets".into(),
        },
        host.clone(),
    )
    .unwrap();
    engine
        .create_space(
            Some("mobile targets".into()),
            "correct horse battery staple".into(),
        )
        .unwrap();
    engine.suspend().unwrap();
    let (entered, waiting) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    *lock(&host.secure_read_gate) = Some(ReadGate {
        key_prefix,
        matches_before_wait,
        entered,
        release: proceed,
    });
    let resuming = thread::spawn({
        let engine = engine.clone();
        move || engine.resume()
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let suspend_started = Instant::now();
    assert!(engine.suspend_with_deadline(0).is_err());
    assert!(suspend_started.elapsed() < Duration::from_secs(1));
    release.send(()).unwrap();
    assert!(matches!(
        resuming.join().unwrap(),
        Err(BindingError::Engine {
            category: BindingErrorCategory::InvalidState,
            ..
        })
    ));
    engine
        .suspend_with_deadline(ENGINE_SHUTDOWN_DEADLINE_MS)
        .unwrap();
    assert_eq!(
        engine.lifecycle_state().unwrap(),
        BindingEngineState::Suspended
    );
    assert!(engine.list_devices().is_err());
    engine.resume().unwrap();
    assert!(engine.list_devices().is_ok());
    let start = Arc::new(Barrier::new(3));
    let closers: Vec<_> = (0..2)
        .map(|_| {
            let engine = engine.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                engine.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
            })
        })
        .collect();
    start.wait();
    for closer in closers {
        closer.join().unwrap().unwrap();
    }
}
