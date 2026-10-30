//! 进程释放终态失败时，登记不能无限等待或越过保留的封口。
#![cfg(feature = "test-process-release")]

use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use uc_observability_runtime::{
    retain_engine_closer, update_engine_closer, DeploymentEnvironment, EngineLogActivity,
    ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    ProcessReleaseResult, ProcessResumeResult, RetainedEngineCloser, RetainedEngineResult,
    RetainedEngineState, SignalResult,
};

struct TerminalCloser {
    entered: mpsc::Sender<()>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl RetainedEngineCloser for TerminalCloser {
    fn wait_accepted_work(&self) {
        self.entered.send(()).expect("signal retained closer");
        let (lock, changed) = &*self.gate;
        let mut released = lock.lock().expect("gate");
        while !*released {
            released = changed.wait(released).expect("gate wait");
        }
    }

    fn retry_close(&self) -> RetainedEngineResult {
        RetainedEngineResult::Released
    }
}

#[test]
fn terminal_retained_engine_fails_waiting_and_later_registrations() {
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (entered, waiting) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let token = retain_engine_closer(
        Arc::new(TerminalCloser {
            entered,
            gate: Arc::clone(&gate),
        }),
        RetainedEngineState::Terminal,
    );
    let release_handle = handle.clone();
    let release = thread::spawn(move || release_handle.release_process(Duration::from_secs(5)));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("release reached closer");

    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let (activity, result) = EngineLogActivity::begin(Duration::MAX);
        sent.send((activity.is_admitted(), result))
            .expect("registration result");
    });
    assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
    *gate.0.lock().expect("gate") = true;
    gate.1.notify_all();

    let (report, _) = release
        .join()
        .expect("release thread")
        .expect("release action");
    assert!(matches!(report.result, ProcessReleaseResult::Failed { .. }));
    assert!(!report.explicit_retry_supported);
    assert_eq!(
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("registration settles"),
        (false, Some(SignalResult::Failed))
    );
    for _ in 0..2 {
        let (activity, result) = EngineLogActivity::begin(Duration::from_secs(1));
        assert!(!activity.is_admitted());
        assert_eq!(result, Some(SignalResult::Failed));
        let (resume, _) = handle
            .resume_process(Duration::ZERO)
            .expect("resume action");
        assert!(matches!(resume, ProcessResumeResult::Failed { .. }));
    }
    assert_eq!(handle.test_retained_engine_count(), 1);

    update_engine_closer(&token, RetainedEngineState::Retryable);
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let (activity, result) = EngineLogActivity::begin(Duration::MAX);
        sent.send((activity, result)).expect("registration result");
    });
    assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
    let (retry, _) = handle
        .retry_process_release(Duration::from_secs(5))
        .expect("retry");
    assert!(matches!(
        retry.result,
        ProcessReleaseResult::Released { .. }
    ));
    assert_eq!(handle.test_retained_engine_count(), 0);
    let (resume, _) = handle
        .resume_process(Duration::from_secs(5))
        .expect("resume");
    assert!(matches!(resume, ProcessResumeResult::Resumed { .. }));
    let (mut activity, result) = received
        .recv_timeout(Duration::from_secs(2))
        .expect("registration after resume");
    assert!(activity.is_admitted());
    assert!(matches!(result, None | Some(SignalResult::Completed)));
    assert!(matches!(
        activity.end(Duration::from_secs(2)),
        None | Some(SignalResult::Completed)
    ));
}
