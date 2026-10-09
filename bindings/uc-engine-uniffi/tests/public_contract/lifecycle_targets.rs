use std::sync::{mpsc, Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    engine_test_guard, lock, BindingConfig, BindingEngineState, BindingError, BindingErrorCategory,
    MemoryHost, MobileEngine, ENGINE_SHUTDOWN_DEADLINE_MS,
};

#[derive(Clone)]
pub(super) struct ReadGate {
    pub(super) key_prefix: &'static str,
    pub(super) matches_before_wait: usize,
    pub(super) entered: mpsc::Sender<()>,
    pub(super) release: Arc<(Mutex<bool>, Condvar)>,
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
        let (released, changed) = self.release.as_ref();
        let released = lock(released);
        if *released {
            return;
        }
        self.entered.send(()).unwrap();
        let (released, _) = changed
            .wait_timeout_while(released, Duration::from_secs(15), |released| !*released)
            .unwrap();
        assert!(*released, "secure storage gate was not released");
    }
}

#[test]
fn a_pause_reaches_the_engine_while_the_previous_mobile_resume_is_waiting() {
    // 首次 KEK 读取打开资料密文；后续会话恢复及成员维护可能并发再次读取。
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
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    *lock(&host.secure_read_gate) = Some(ReadGate {
        key_prefix,
        matches_before_wait,
        entered,
        release: release.clone(),
    });
    let resuming = thread::spawn({
        let engine = engine.clone();
        move || engine.resume()
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let suspend_started = Instant::now();
    assert!(engine.suspend_with_deadline(0).is_err());
    assert!(suspend_started.elapsed() < Duration::from_secs(1));
    // 零期限只保证通知已入队；同一生命周期通道的查询返回后，暂停才已转交 Engine。
    // 密钥读取继续保持阻塞，确保这里验证的是暂停打断恢复，而非两者抢跑。
    assert_eq!(
        engine.lifecycle_state().unwrap(),
        BindingEngineState::Quiesced
    );
    *lock(&release.0) = true;
    release.1.notify_all();
    let resume_result = resuming.join().unwrap();
    assert!(
        matches!(
            resume_result,
            Err(BindingError::Engine {
                category: BindingErrorCategory::InvalidState,
                ..
            })
        ),
        "unexpected resume result: {resume_result:?}"
    );
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
