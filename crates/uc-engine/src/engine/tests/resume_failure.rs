use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{Engine, EngineErrorCategory, EngineState, FakeRuntime, Operation};

#[tokio::test]
async fn failed_resume_does_not_prove_safe_suspension_or_skip_the_next_cleanup() {
    let runtime = Arc::new(FakeRuntime::default());
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 16);
    engine.suspend().await.unwrap();
    runtime.fail_resume.store(true, Ordering::SeqCst);
    engine.resume().await.unwrap_err();
    assert_ne!(engine.lifecycle_state().await, EngineState::Suspended);
    assert_eq!(
        engine
            .execute(Operation::ListDevices)
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::InvalidState
    );

    runtime.fail_suspend.store(true, Ordering::SeqCst);
    engine.suspend().await.unwrap_err();
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 2);
    assert_ne!(engine.lifecycle_state().await, EngineState::Suspended);

    runtime.fail_suspend.store(false, Ordering::SeqCst);
    engine.suspend().await.unwrap();
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 3);
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn resume_can_retry_the_complete_runtime_action_after_a_failed_attempt() {
    let runtime = Arc::new(FakeRuntime::default());
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 16);
    engine.suspend().await.unwrap();
    runtime.fail_resume.store(true, Ordering::SeqCst);
    engine.resume().await.unwrap_err();
    runtime.fail_resume.store(false, Ordering::SeqCst);
    engine.resume().await.unwrap();
    assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 2);
    assert_eq!(engine.lifecycle_state().await, EngineState::Running);
    engine.execute(Operation::ListDevices).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
}
